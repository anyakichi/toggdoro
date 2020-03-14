extern crate clap;
#[macro_use]
extern crate failure;
#[macro_use]
extern crate lazy_static;
extern crate lettre;
extern crate lettre_email;
extern crate notify_rust;
extern crate regex;
extern crate reqwest;
extern crate slack_hook;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate tinytemplate;
extern crate toml;

use std::fs::File;
use std::io::prelude::*;
use std::io::BufReader;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::RwLock;
use std::{env, fs, process, thread, time};

use chrono::{DateTime, Local};
use clap::{App, Arg};
use failure::Error;
use lettre::{SendmailTransport, Transport};
use lettre_email::Email;
use regex::Regex;
use slack_hook::{PayloadBuilder, Slack};
use tinytemplate::TinyTemplate;

use crate::toggl::TimeEntry;

#[derive(Debug, Default, Deserialize)]
struct Config {
    version: u8,
    toggl_token: String,
    socket: Option<String>,

    #[serde(default)]
    notification: NotificationConfig,

    #[serde(default)]
    pomodoro: PomodoroConfig,
}

#[derive(Debug, Default, Deserialize)]
struct NotificationConfig {
    #[serde(default)]
    dbus: bool,

    mail: Option<String>,

    slack: Option<String>,
}

#[derive(Serialize)]
struct Context {
    count: u32,
    remaining_time: String,
    remaining_time_abs: String,
    task: String,
}

#[derive(Debug, Deserialize)]
struct PomodoroConfig {
    #[serde(default = "default_pomodoro_min")]
    pomodoro_min: u32,

    #[serde(default = "default_short_break_min")]
    short_break_min: u32,

    #[serde(default = "default_long_break_min")]
    long_break_min: u32,

    #[serde(default = "default_long_break_after")]
    long_break_after: u32,
}

fn default_pomodoro_min() -> u32 {
    25
}
fn default_short_break_min() -> u32 {
    5
}
fn default_long_break_min() -> u32 {
    15
}
fn default_long_break_after() -> u32 {
    4
}

impl Default for PomodoroConfig {
    fn default() -> Self {
        Self {
            pomodoro_min: default_pomodoro_min(),
            short_break_min: default_short_break_min(),
            long_break_min: default_long_break_min(),
            long_break_after: default_long_break_after(),
        }
    }
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
    ntnotifications: u32,
    mode: PomodoroMode,
    finish_time: DateTime<Local>,
    task_finish_time: Option<DateTime<Local>>,
}

impl Default for PomodoroState {
    fn default() -> Self {
        Self {
            npomodoros: 0,
            nnotifications: 0,
            ntnotifications: 0,
            mode: PomodoroMode::Idle,
            finish_time: Local::now(),
            task_finish_time: None,
        }
    }
}

mod toggl {
    use chrono::{DateTime, Local};
    use failure::Error;

    pub struct Toggl {
        token: String,
        client: reqwest::Client,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct TimeEntry {
        pub id: u64,
        pub guid: String,
        pub wid: u32,
        pub pid: Option<u32>,
        pub billable: bool,
        pub start: DateTime<Local>,
        pub stop: Option<DateTime<Local>>,
        pub duration: i32,
        pub description: String,
        #[serde(default)]
        pub tags: Vec<String>,
        pub duronly: bool,
        pub at: DateTime<Local>,
        pub uid: u32,
    }

    #[derive(Debug, Deserialize)]
    pub struct TimeEntryData {
        pub data: Option<TimeEntry>,
    }

    pub fn new(token: String) -> Toggl {
        Toggl {
            token,
            client: reqwest::Client::new(),
        }
    }

    pub mod api {
        use crate::toggl::*;

        pub fn time_entries(toggl: &Toggl) -> Result<Vec<TimeEntry>, Error> {
            let mut res = toggl
                .client
                .get("https://www.toggl.com/api/v8/time_entries")
                .basic_auth(&toggl.token, Some("api_token"))
                .send()?;
            let entries = res.json::<Vec<TimeEntry>>()?;
            Ok(entries)
        }
    }
}

lazy_static! {
    static ref CONFIG: RwLock<Config> = RwLock::new(Default::default());
    static ref POMODORO_STATE: RwLock<PomodoroState> = RwLock::new(Default::default());
}

fn notify_by_dbus(config: &Config, msg: &str) -> Result<(), Error> {
    if config.notification.dbus {
        notify_rust::Notification::new()
            .summary("Toggdoro")
            .body(msg)
            .show()
            .map_err(|e| format_err!("{}", e))?;
    }
    Ok(())
}

fn notify_by_slack(config: &Config, msg: &str) -> Result<(), Error> {
    if let Some(url) = config.notification.slack.as_ref() {
        //let emoji = if mode == "Work" {
        //    ":tomato:"
        //} else {
        //    ":coffee:"
        //};
        let slack = Slack::new(url as &str).map_err(|e| format_err!("{}", e))?;
        let p = PayloadBuilder::new()
            .username("toggdoro")
            //.icon_emoji(emoji)
            .text(msg)
            .build()
            .map_err(|e| format_err!("{}", e))?;

        slack.send(&p).map_err(|e| format_err!("{}", e))?;
    }
    Ok(())
}

fn notify_by_mail(config: &Config, msg: &str) -> Result<(), Error> {
    if let Some(to) = config.notification.mail.as_ref() {
        let email = Email::builder()
            .from("toggdoro@sopht.jp")
            .to(to as &str)
            .subject(msg)
            .text("")
            .build()?;

        let mut mailer = SendmailTransport::new();
        mailer.send(email.into())?;
    }
    Ok(())
}

fn mode_of_entry(entry: &TimeEntry) -> PomodoroMode {
    if entry.description == "Pomodoro Break" {
        return PomodoroMode::Break;
    }
    if entry.tags.iter().any(|x| x == "pomodoro-break") {
        PomodoroMode::Break
    } else {
        PomodoroMode::Work
    }
}

fn task_min(entry: &TimeEntry) -> Result<Option<u32>, Error> {
    let re = Regex::new(r"^(\d+)min$")?;
    for tag in &entry.tags {
        if let Some(cap) = re.captures(&tag) {
            return Ok(Some(cap[1].parse()?));
        }
    }
    Ok(None)
}

fn update(toggl: &toggl::Toggl) -> Result<(), Error> {
    let config = CONFIG.read().unwrap();
    let pomodoro_config = &config.pomodoro;
    let mut entries = toggl::api::time_entries(&toggl)?;
    let mut state = POMODORO_STATE.write().unwrap();
    let mut history: Vec<(PomodoroMode, i32)> = Vec::new();

    state.mode = PomodoroMode::Idle;

    if let Some(latest_entry) = entries.pop() {
        if latest_entry.duration >= 0 {
            return Ok(());
        }
        let mut last_start = &latest_entry.start;
        let mut extra_task_duration = 0;
        state.mode = mode_of_entry(&latest_entry);

        if state.mode == PomodoroMode::Work {
            for x in entries.iter().rev() {
                if mode_of_entry(x) == PomodoroMode::Break {
                    continue;
                }
                if latest_entry.description == x.description
                    && latest_entry.pid == x.pid
                    && latest_entry.tags == x.tags
                {
                    extra_task_duration += x.duration;
                } else {
                    break;
                }
            }
        }

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
                Some(ref mut v) if v.0 == mode => **v = (v.0, v.1 + x.duration),
                _ => history.push((mode, x.duration)),
            }

            if let Some(&(PomodoroMode::Break, d)) = history.last() {
                if d >= (pomodoro_config.long_break_min as i32 * 60) {
                    history.pop();
                    break;
                }
            }

            last_start = &x.start;
        }
        state.npomodoros = (history.len() / 2 + 1) as u32;
        let mut duration = {
            if mode_of_entry(&latest_entry) == PomodoroMode::Break {
                if state.npomodoros >= pomodoro_config.long_break_after {
                    pomodoro_config.long_break_min as i32 * 60
                } else {
                    pomodoro_config.short_break_min as i32 * 60
                }
            } else {
                pomodoro_config.pomodoro_min as i32 * 60
            }
        };
        if let Some(v) = history.first() {
            if v.0 == mode_of_entry(&latest_entry) {
                duration -= v.1;
            }
        }
        state.finish_time = latest_entry.start + chrono::Duration::seconds(duration as i64);
        state.task_finish_time = task_min(&latest_entry)?.map(|x| {
            latest_entry.start
                + chrono::Duration::seconds(x as i64 * 60 - extra_task_duration as i64)
        });

        // notification
        let now = Local::now();
        let duration = state.finish_time - now;
        let dur_secs = duration.num_seconds();

        if dur_secs < 0 {
            let msg = {
                if mode_of_entry(&latest_entry) == PomodoroMode::Break {
                    format!("Work {} min", pomodoro_config.pomodoro_min)
                } else {
                    format!(
                        "Break {} min",
                        if state.npomodoros >= pomodoro_config.long_break_after {
                            pomodoro_config.long_break_min
                        } else {
                            pomodoro_config.short_break_min
                        }
                    )
                }
            };

            if (state.nnotifications == 0)
                || (state.nnotifications == 1 && dur_secs < -300)
                || (state.nnotifications == 2 && dur_secs < -1800)
            {
                notify_by_dbus(&config, &msg)?;
                notify_by_slack(&config, &msg)?;
                notify_by_mail(&config, &msg)?;
                state.nnotifications += 1;
            }
            state.ntnotifications = 0;
        } else {
            state.nnotifications = 0;

            if let Some(task_finish_time) = state.task_finish_time {
                let task_duration = task_finish_time - now;
                let task_dur_secs = task_duration.num_seconds();

                if (state.ntnotifications == 0 && task_dur_secs < 0)
                    || (state.ntnotifications == 1 && task_dur_secs < -300)
                    || (state.ntnotifications == 2 && task_dur_secs < -1800)
                {
                    notify_by_dbus(&config, "Switch to the next task")?;
                    notify_by_slack(&config, "Switch to the next task")?;
                    notify_by_mail(&config, "Switch to the next task")?;
                    state.ntnotifications += 1;
                }
            } else {
                state.ntnotifications = 0;
            }
        }
    }
    Ok(())
}

fn monitor() {
    let interval = time::Duration::from_secs(3);
    let toggl = {
        let config = CONFIG.read().unwrap();
        toggl::new(config.toggl_token.to_string())
    };
    loop {
        if let Err(e) = update(&toggl) {
            println!("{}", e);
        }
        thread::sleep(interval);
    }
}

fn handle_connection(mut stream: UnixStream) -> Result<(), Error> {
    let mut tt = TinyTemplate::new();
    tt.add_template("Idle", "idle")?;
    tt.add_template(
        "Work",
        "<span foreground=\"#ff6347\"> {count}[{remaining_time_abs}{task}]</span>",
    )?;
    tt.add_template(
        "Break",
        "<span foreground=\"#47beff\"> {count}[{remaining_time_abs}{task}]</span>",
    )?;
    tt.add_template("overWork", "<span foreground=\"#ff6347\"> {count}[<span foreground=\"#ffffff\" background=\"#cc4f39\">{remaining_time_abs}</span>{task}]</span>")?;
    tt.add_template("overBreak", "<span foreground=\"#47beff\"> {count}[<span foreground=\"#ffffff\" background=\"#397dcc\">{remaining_time_abs}</span>{task}]</span>")?;
    tt.add_template("WorkTask", "|{remaining_time_abs}")?;
    tt.add_template("BreakTask", "|{remaining_time_abs}")?;
    tt.add_template(
        "overWorkTask",
        "|<span foreground=\"#ffffff\" background=\"#cc4f39\">{remaining_time_abs}</span>",
    )?;
    tt.add_template(
        "overBreakTask",
        "|<span foreground=\"#ffffff\" background=\"#397dcc\">{remaining_time_abs}</span>",
    )?;

    let state = POMODORO_STATE.read().unwrap();
    match state.mode {
        PomodoroMode::Idle => writeln!(stream, "idle")?,
        mode => {
            let now = Local::now();

            let task = if let Some(finish_time) = state.task_finish_time {
                let duration = finish_time - now;
                let timeover = duration.num_seconds() < 0;
                let template = if timeover {
                    format!("over{:?}Task", mode)
                } else {
                    format!("{:?}Task", mode)
                };
                let mins = duration.num_minutes();
                let secs = duration.num_seconds().abs() % 60;

                let context = Context {
                    count: state.npomodoros,
                    remaining_time: format!("{:02}:{:02}", mins, secs),
                    remaining_time_abs: format!("{:02}:{:02}", mins.abs(), secs),
                    task: "".to_string(),
                };

                tt.render(&template, &context).unwrap()
            } else {
                "".to_string()
            };

            let duration = state.finish_time - now;
            let timeover = duration.num_seconds() < 0;
            let template = if timeover {
                format!("over{:?}", mode)
            } else {
                format!("{:?}", mode)
            };
            let mins = duration.num_minutes();
            let secs = duration.num_seconds().abs() % 60;

            let context = Context {
                count: state.npomodoros,
                remaining_time: format!("{:02}:{:02}", mins, secs),
                remaining_time_abs: format!("{:02}:{:02}", mins.abs(), secs),
                task: task,
            };

            writeln!(stream, "{}", tt.render(&template, &context)?)?;
        }
    };

    Ok(())
}

fn load_config(path: &str) -> Result<(), Error> {
    let mut c = CONFIG.write().unwrap();

    let file = File::open(path)?;
    let mut buf_reader = BufReader::new(file);
    let mut contents = String::new();
    buf_reader.read_to_string(&mut contents)?;
    let config = toml::from_str(&contents)?;
    *c = config;

    Ok(())
}

fn main() -> Result<(), Error> {
    let matches = App::new("toggdoro")
        .version("0.1")
        .author("INAJIMA Daisuke <inajima@sopht.jp>")
        .about("Pomodoro timer with toggl")
        .arg(
            Arg::with_name("config")
                .short("c")
                .long("config")
                .value_name("FILE")
                .help("Sets config file")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("socket")
                .short("s")
                .long("socket")
                .value_name("SOCKET")
                .help("Sets UNIX domain socket path")
                .takes_value(true),
        )
        .get_matches();

    let home = env::var("HOME").unwrap_or(".".to_string());
    let config_path = matches
        .value_of("config")
        .map(|x| x.to_string())
        .unwrap_or(home.to_string() + "/.config/toggdoro/config.toml");

    load_config(&config_path)?;

    let path = env::var("XDG_RUNTIME_DIR")
        .map(|x| x.to_string() + "/toggdoro.sock")
        .unwrap_or(home.to_string() + "/.toggdoro.sock");

    let listener = UnixListener::bind(&path)?;

    let _ = unsafe {
        signal_hook::register(signal_hook::SIGINT, move || {
            fs::remove_file(&path).unwrap();
            process::exit(130);
        })
    }?;

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

    Ok(())
}
