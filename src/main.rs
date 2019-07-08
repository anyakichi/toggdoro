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

#[derive(Debug, Deserialize, Serialize)]
struct Config {
    version: u8,
    toggl_token: String,
    socket: Option<String>,
    mail_notification: Option<String>,
    desktop_notification: Option<bool>,
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
    mode: PomodoroMode,
    npomodoros: u32,
    finish_time: DateTime<Local>,
}

const POMODORO_WORK_MIN: i32 = 25;
const POMODORO_SHORT_BREAK_MIN: i32 = 5;
const POMODORO_LONG_BREAK_MIN: i32 = 15;

lazy_static! {
    static ref CONFIG: RwLock<Config> =
        RwLock::new(Config {
            version: 0,
            toggl_token: "".to_string(),
            socket: None,
            mail_notification: None,
            desktop_notification: None,
        });

    static ref POMODORO_STATE: RwLock<PomodoroState> =
        RwLock::new(PomodoroState {
            mode: PomodoroMode::Idle,
            npomodoros: 0,
            finish_time: Local::now()
        });
}


fn notify(mode: &str, min: i32) {
    notify_rust::Notification::new()
        .summary("Toggdoro")
        .body(&format!("{} {} minutes", mode, min))
        .show().unwrap();
}

fn sendmail(subject: &str, body: &str) -> Result<(), Error> {
    let config = CONFIG.read().unwrap();

    if let Some(ref to) = config.mail_notification {
        let email = Email::builder()
            .to(to as &str)
            .subject(subject)
            .text(body)
            .build()
            .unwrap();

        let mut mailer = SendmailTransport::new();
        mailer.send(email.into())?;
    }
    Ok(())
}

fn api_time_entries() -> Result<Vec<TimeEntry>, Error> {
    let config = CONFIG.read().unwrap();

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

fn monitor() {
    let interval = time::Duration::from_secs(3);
    let mut num_dnotify = 0;    // desktop notification
    let mut num_mnotify = 0;    // mail notification
    loop {
        match api_time_entries() {
            Ok(mut entries) => {
                let mut state = POMODORO_STATE.write().unwrap();
                let mut history: Vec<(PomodoroMode, i32)> = Vec::new();

                if let Some(latest_entry) = entries.pop() {
                    if latest_entry.duration >= 0 {
                        state.mode = PomodoroMode::Idle;
                    } else {
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
                                if d >= (POMODORO_LONG_BREAK_MIN * 60) {
                                    history.pop();
                                    break;
                                }
                            }

                            last_start = &x.start;
                        }
                        state.npomodoros = (history.len() / 2 + 1) as u32;
                        let mut duration = {
                            if mode_of_entry(&latest_entry) == PomodoroMode::Break {
                                if state.npomodoros >= 4 {
                                    POMODORO_LONG_BREAK_MIN * 60
                                } else {
                                    POMODORO_SHORT_BREAK_MIN * 60
                                }
                            } else {
                                POMODORO_WORK_MIN * 60
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
                                    ("Work", POMODORO_WORK_MIN)
                                } else {
                                    if state.npomodoros >= 4 {
                                        ("Break", POMODORO_LONG_BREAK_MIN)
                                    } else {
                                        ("Break", POMODORO_SHORT_BREAK_MIN)
                                    }
                                }
                            };

                            if (num_dnotify == 0) ||
                               (num_dnotify == 1 && dur_secs < -300) ||
                               (num_dnotify == 2 && dur_secs < -1800) {
                                   notify(next_mode, next_min);
                                   num_dnotify += 1;
                            }

                            if (num_mnotify == 0 && dur_secs < -30) ||
                               (num_mnotify == 1 && dur_secs < -300) ||
                               (num_mnotify == 2 && dur_secs < -1800) {
                                   sendmail(&format!("Pomodoro: {} {} minutes",
                                            next_mode, next_min), "").unwrap();
                                   num_mnotify += 1;
                            }
                        } else {
                            num_dnotify = 0;
                            num_mnotify = 0;
                        }
                    }
                } else {
                    state.mode = PomodoroMode::Idle;
                }
            }
            Err(error) => {
                println!("{}", error);
            }
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
            .arg(Arg::with_name("desktop_notification")
                    .short("d")
                    .long("desktop_notification")
                    .help("Enable desktop notification")
                    .takes_value(false))
            .arg(Arg::with_name("mail_notification")
                    .short("m")
                    .long("mail_notification")
                    .value_name("ADDRESS")
                    .help("Sets mail address to notify")
                    .takes_value(true))
            .arg(Arg::with_name("socket")
                    .short("s")
                    .long("socket")
                    .value_name("SOCKET")
                    .help("Sets UNIX domain socket path")
                    .takes_value(true))
            .arg(Arg::with_name("toggl_token")
                    .short("t")
                    .long("toggl_token")
                    .value_name("TOKEN")
                    .help("Sets API token of toggl")
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
            .unwrap_or(home.to_string() + ".toggdoro.sock");

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
