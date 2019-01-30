extern crate slack_hook;
extern crate clap;
extern crate toml;

extern crate lettre;
extern crate lettre_email;

extern crate reqwest;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate serde_json;

#[macro_use]
extern crate lazy_static;

#[macro_use]
extern crate failure;

extern crate notify_rust;

use std::{env, fs, thread, process, time};
use std::fs::File;
use std::io::BufReader;
use std::io::prelude::*;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::RwLock;

use chrono::{DateTime, Local};
use clap::{Arg, App};
use failure::Error;
use lettre::{Transport, SendmailTransport};
use lettre_email::Email;
use reqwest::Client;
use slack_hook::{Slack, PayloadBuilder};

#[derive(Debug, Deserialize)]
struct Config {
    version: u8,
    toggl_token: String,
    socket: Option<String>,
    notification: Option<NotificationConfig>,
    pomodoro: Option<PomodoroConfig>,
}

#[derive(Debug, Deserialize)]
struct NotificationConfig {
    dbus: Option<bool>,
    mail: Option<String>,
    slack: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PomodoroConfig {
    pomodoro_min: Option<u32>,
    short_break_min: Option<u32>,
    long_break_min: Option<u32>,
    long_break_after: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TimeEntry {
    id: u64,
    guid: String,
    wid: u32,
    pid: Option<u32>,
    billable: bool,
    start: DateTime<Local>,
    stop: Option<DateTime<Local>>,
    duration: i32,
    description: String,
    tags: Option<Vec<String>>,
    duronly: bool,
    at: DateTime<Local>,
    uid: u32,
}

#[derive(Debug, Deserialize)]
struct TimeEntryData {
    data: Option<TimeEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum PomodoroMode {
    Idle,
    Work,
    Break,
}

struct PomodoroState {
    npomodoros: u32,
    nnotifications: u32,
    mode: PomodoroMode,
    finish_time: DateTime<Local>,
}

lazy_static! {
    static ref CONFIG: RwLock<Config> =
        RwLock::new(Config {
            version: 0,
            toggl_token: "".to_string(),
            socket: None,
            notification: None,
            pomodoro: None,
        });

    static ref POMODORO_STATE: RwLock<PomodoroState> =
        RwLock::new(PomodoroState {
            npomodoros: 0,
            nnotifications: 0,
            mode: PomodoroMode::Idle,
            finish_time: Local::now()
        });
}


fn notify_by_dbus(config: &Config, mode: &str, min: u32) -> Result<(), Error> {
    if config.notification.as_ref().and_then(|n| n.dbus).unwrap_or(false) {
        notify_rust::Notification::new()
            .summary("Toggdoro")
            .body(&format!("{} {} minutes", mode, min))
            .show().map_err(|e| format_err!("{}", e))?;
    }
    Ok(())
}

fn notify_by_slack(config: &Config, mode: &str, min: u32) -> Result<(), Error> {
    if let Some(url) = config.notification.as_ref()
                        .and_then(|n| n.slack.as_ref()) {
        let emoji = if mode == "Work" {
            ":tomato:"
        } else {
            ":coffee:"
        };
        let slack = Slack::new(url as &str).map_err(|e| format_err!("{}", e))?;
        let p = PayloadBuilder::new()
            .username("toggdoro")
            .icon_emoji(emoji)
            .text(format!("{} {} minutes", mode, min))
            .build()
            .map_err(|e| format_err!("{}", e))?;

        slack.send(&p).map_err(|e| format_err!("{}", e))?;
    }
    Ok(())
}

fn notify_by_mail(config: &Config, mode: &str, min: u32) -> Result<(), Error> {
    if let Some(to) = config.notification.as_ref()
                            .and_then(|n| n.mail.as_ref()) {
        let email = Email::builder()
            .from("toggdoro@sopht.jp")
            .to(to as &str)
            .subject(format!("Pomodoro: {} {} minutes", mode, min))
            .text("")
            .build()?;

        let mut mailer = SendmailTransport::new();
        mailer.send(email.into())?;
    }
    Ok(())
}

fn api_time_entries(config: &Config) -> Result<Vec<TimeEntry>, Error> {
    let mut res = Client::new()
        .get("https://www.toggl.com/api/v8/time_entries")
        .basic_auth(&config.toggl_token, Some("api_token"))
        .send()?;
    let entries = res.json::<Vec<TimeEntry>>()?;
    Ok(entries)
}

fn mode_of_entry(entry: &TimeEntry) -> PomodoroMode {
    if entry.description == "Pomodoro Break" {
        return PomodoroMode::Break
    }
    entry.tags.as_ref()
        .filter(|tags| tags.contains(&"pomodoro-break".to_string()))
        .map_or(PomodoroMode::Work, |_| PomodoroMode::Break)
}

fn update() -> Result<(), Error> {
    let config = CONFIG.read().unwrap();
    let pomodoro_min =
        config.pomodoro.as_ref().and_then(|x| x.pomodoro_min).unwrap_or(25);
    let short_break_min =
        config.pomodoro.as_ref().and_then(|x| x.short_break_min).unwrap_or(5);
    let long_break_min =
        config.pomodoro.as_ref().and_then(|x| x.long_break_min).unwrap_or(15);
    let long_break_after =
        config.pomodoro.as_ref().and_then(|x| x.long_break_after).unwrap_or(4);
    let mut entries = api_time_entries(&config)?;
    let mut state = POMODORO_STATE.write().unwrap();
    let mut history: Vec<(PomodoroMode, i32)> = Vec::new();

    state.mode = PomodoroMode::Idle;

    if let Some(latest_entry) = entries.pop() {
        if latest_entry.duration >= 0 {
            return Ok(());
        }
        let mut last_start = &latest_entry.start;
        state.mode = mode_of_entry(&latest_entry);

        for x in entries.iter().rev() {
            let mode = mode_of_entry(x);

            if let Some(stop) = x.stop {
                if (*last_start - stop).num_seconds() > 120 {
                    break;
                }
            } else {
                break;
            }

            match history.last_mut() {
                Some(ref mut v) if v.0 == mode => {
                    **v = (v.0, v.1 + x.duration)
                }
                _ => {
                    history.push((mode, x.duration))
                }
            }

            if let Some(&(PomodoroMode::Break, d)) = history.last() {
                if d >= (long_break_min as i32 * 60) {
                    history.pop();
                    break;
                }
            }

            last_start = &x.start;
        }
        state.npomodoros = (history.len() / 2 + 1) as u32;
        let mut duration = {
            if mode_of_entry(&latest_entry) == PomodoroMode::Break {
                if state.npomodoros >= long_break_after {
                    long_break_min as i32 * 60
                } else {
                    short_break_min as i32 * 60
                }
            } else {
                pomodoro_min as i32 * 60
            }
        };
        if let Some(v) = history.first() {
            if v.0 == mode_of_entry(&latest_entry) {
                duration -= v.1;
            }
        }
        state.finish_time = latest_entry.start +
            chrono::Duration::seconds(duration as i64);

        // notification
        let duration = state.finish_time - Local::now();
        let dur_secs = duration.num_seconds();

        if dur_secs < 0 {
            let (next_mode, next_min) = {
                if mode_of_entry(&latest_entry) == PomodoroMode::Break {
                    ("Work", pomodoro_min)
                } else {
                    if state.npomodoros >= long_break_after {
                        ("Break", long_break_min)
                    } else {
                        ("Break", short_break_min)
                    }
                }
            };

            if (state.nnotifications == 0) ||
                (state.nnotifications == 1 && dur_secs < -300) ||
                (state.nnotifications == 2 && dur_secs < -1800) {
                        notify_by_dbus(&config, next_mode, next_min)?;
                        notify_by_slack(&config, next_mode, next_min)?;
                        notify_by_mail(&config, next_mode, next_min)?;
                        state.nnotifications += 1;
            }
        } else {
            state.nnotifications = 0
        }
    }
    Ok(())
}

fn monitor() {
    let interval = time::Duration::from_secs(3);
    loop {
        if let Err(e) = update() {
            println!("{}", e);
        }
        thread::sleep(interval);
    }
}

fn handle_connection(mut stream: UnixStream) {
    let state = POMODORO_STATE.read().unwrap();
    match state.mode {
        PomodoroMode::Idle => writeln!(stream, "idle"),
        mode => {
            let duration = state.finish_time - Local::now();
            let (fgbg, min, sec) =
                if duration.num_seconds() >= 0 {
                    ("fg", duration.num_minutes(), duration.num_seconds() % 60)
                } else {
                    ("bg", -duration.num_minutes(), -duration.num_seconds() % 60)
                };
            let color =
                if mode == PomodoroMode::Work {
                    "colour203"
                } else {
                    "colour75"
                };
            writeln!(stream, "#[{}={}]{}|{:02}:{:02}#[default]",
                     fgbg, color, state.npomodoros, min, sec)
        }
    }.unwrap();
}

fn load_config(path: &str) {
    let mut c = CONFIG.write().unwrap();

    let file = File::open(path).unwrap();
    let mut buf_reader = BufReader::new(file);
    let mut contents = String::new();
    buf_reader.read_to_string(&mut contents).unwrap();
    let config = toml::from_str(&contents).unwrap();
    *c = config;
}

fn main() {
    let matches =
        App::new("toggdoro")
            .version("0.1")
            .author("INAJIMA Daisuke <inajima@sopht.jp>")
            .about("Pomodoro timer with toggl")
            .arg(Arg::with_name("config")
                    .short("c")
                    .long("config")
                    .value_name("FILE")
                    .help("Sets config file")
                    .takes_value(true))
            .arg(Arg::with_name("socket")
                    .short("s")
                    .long("socket")
                    .value_name("SOCKET")
                    .help("Sets UNIX domain socket path")
                    .takes_value(true))
            .get_matches();

    let home = env::var("HOME").unwrap_or(".".to_string());
    let config_path = matches.value_of("config")
        .map(|x| x.to_string())
        .unwrap_or(home.to_string() + "/.config/toggdoro/config.toml");

    load_config(&config_path);

    let path =
        env::var("XDG_RUNTIME_DIR")
            .map(|x| x.to_string() + "/toggdoro.sock")
            .unwrap_or(home.to_string() + "/.toggdoro.sock");

    let listener = UnixListener::bind(&path).unwrap();

    let _ = unsafe {
        signal_hook::register(signal_hook::SIGINT,
                              move || {
                                  fs::remove_file(&path).unwrap();
                                  process::exit(130);
                              })
    }.unwrap();

    let _ = thread::spawn(|| monitor());

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                thread::spawn(|| handle_connection(stream));
            }
            Err(err) => {
                println!("accept failed: {:?}", err);
            }
        }
    }
}
