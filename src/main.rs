use futures::prelude::*;
use irc::client::prelude::*;
use std::fs;
use tokio::runtime;
use tokio::sync::mpsc;

use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
struct RawConfig {
    seika: Option<RawSeikaConfig>,
}

#[derive(Debug, Deserialize)]
struct RawSeikaConfig {
    url: Option<String>,
    username: Option<String>,
    password: Option<String>,
}

struct SeikaConfig {
    url: url::Url,
    username: String,
    password: String,
}

#[derive(Debug)]
struct IrcMessage {
    from: String,
    message: String,
}

fn fetch_raw_seika_config() -> RawSeikaConfig {
    let mut config: RawConfig =
        toml::from_str(&*fs::read_to_string("Config.toml").expect("failed to read config file!"))
            .expect("failed to parse config file!");
    if let None = config.seika {
        config.seika = Some(RawSeikaConfig {
            url: None,
            username: None,
            password: None,
        });
    }
    config.seika.unwrap()
}

fn parse_seika_url(raw: &Option<String>) -> url::Url {
    let mut parsed_url = url::Url::parse(&*raw.as_ref().unwrap_or(&"http://localhost:7180".into()))
        .expect("failed to parse seika http server url.");
    assert!(!parsed_url.cannot_be_a_base());
    if parsed_url.scheme().is_empty() {
        parsed_url
            .set_scheme("http")
            .expect("failed to set seika url scheme");
    }
    if parsed_url.host() == None {
        parsed_url
            .set_host(Some("localhost"))
            .expect("failed to set seika url host");
    }
    if parsed_url.port() == None {
        parsed_url
            .set_port(Some(7180))
            .expect("failed to set seika url port.");
    }
    parsed_url
}

fn seika_config_from(raw: RawSeikaConfig) -> SeikaConfig {
    let url = parse_seika_url(&raw.url);
    let username = raw.username.unwrap_or("SeikaServerUser".into());
    let password = raw.password.unwrap_or("SeikaServerPassword".into());
    SeikaConfig {
        url: url,
        username: username,
        password: password,
    }
}

async fn run_irc_client(
    sender: mpsc::Sender<IrcMessage>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut client = irc::client::Client::new("Config.toml").await?;
    client.identify()?;
    let mut stream = client.stream()?;

    while let Some(Ok(message)) = stream.next().await {
        let prefix = message.prefix;
        match message.command {
            Command::PING(server1, _server2) => {
                client.send_pong(server1)?;
            }
            Command::PRIVMSG(_channel, text) => {
                if let Some(Prefix::Nickname(from, _, _)) = prefix {
                    sender.try_send(IrcMessage {
                        from: from,
                        message: text,
                    })?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct SeikaVoice {
    cid: u16,
    name: Option<String>,
    platform: Option<String>,
    prod: Option<String>,
}

async fn voices(
    client: &reqwest::Client,
    config: &SeikaConfig,
) -> std::result::Result<Vec<u16>, Box<dyn std::error::Error>> {
    let resp = client
        .get(config.url.join("AVATOR2")?.as_str())
        .basic_auth(&config.username, Some(&config.password))
        .send()
        .await?;
    let body: Vec<SeikaVoice> = resp.json().await?;
    Ok(body.iter().map(|v| v.cid).collect())
}

#[derive(Debug, Serialize, Deserialize)]
struct SeikaSay {
    talktext: String,
    effects: Option<SeikaSayEffects>,
    emotions: Option<SeikaSayEmotions>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SeikaSayEffects {
    speed: f64,
    volume: f64,
    pitch: f64,
    intonation: f64,
}

#[derive(Debug, Serialize, Deserialize)]
struct SeikaSayEmotions {
    #[serde(rename = "怒り")]
    anger: f64,
    #[serde(rename = "喜び")]
    joy: f64,
    #[serde(rename = "悲しみ")]
    sadness: f64,
}

async fn run_seikasay_loop(
    config: SeikaConfig,
    mut receiver: mpsc::Receiver<IrcMessage>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let cids = voices(&client, &config).await?;
    println!("cids: {:?}", cids);
    assert!(cids.len() > 0, "at least 1 voice!");
    while let Some(irc_message) = receiver.recv().await {
        println!("{}: {}", irc_message.from, irc_message.message);
        let resp = client
            .post(
                config
                    .url
                    .join("PLAYASYNC2/")?
                    .join(&*cids[0].to_string())?
                    .as_str(),
            )
            .basic_auth(&config.username, Some(&config.password))
            .json(&SeikaSay {
                talktext: format!("＞ {}: {}", irc_message.from, irc_message.message),
                effects: None,
                emotions: None,
            })
            .send()
            .await;
        //println!(">> {:?}", resp);
    }
    Ok(())
}

fn main() -> () {
    let current_thread_runtime = runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build runtime...");
    current_thread_runtime.block_on(async {
        let (sender, receiver) = mpsc::channel::<IrcMessage>(32);
        let irc_task = tokio::spawn(async move {
            run_irc_client(sender).await.expect("irc client crashed");
        });
        let seika_conf = seika_config_from(fetch_raw_seika_config());
        let seika_task = tokio::spawn(async move {
            run_seikasay_loop(seika_conf, receiver)
                .await
                .expect("seika client crashed");
        });
        let _ = futures::join!(irc_task, seika_task);
        ()
    })
}
