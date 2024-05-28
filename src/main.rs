use std::io::prelude::*;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, RwLock};
use std::{env, fs, process, thread, time};

use chrono::{DateTime, Local};
use clap::{App, Arg};
use failure::Error;
use handlebars::Handlebars;
use lazy_static::lazy_static;
use regex::Regex;
use serde_derive::Serialize;
use signal_hook::{iterator::Signals, SIGINT, SIGTERM};

use toggdoro::config::{Config, CONFIG};
use toggdoro::notifier::dbus::DBusNotifier;
use toggdoro::notifier::mail::MailNotifier;
use toggdoro::notifier::slack::SlackNotifier;
use toggdoro::notifier::Notifier;
use toggdoro::pomodoro::PomodoroMode;
use toggdoro::toggl::{TimeEntry, Toggl};

struct PomodoroState {
    npomodoros: u32,
    nnotifications: u32,
    ntnotifications: u32,
    mode: PomodoroMode,
    description: String,
    project: String,
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
            description: "".to_string(),
            project: "".to_string(),
            finish_time: Local::now(),
            task_finish_time: None,
        }
    }
}

#[derive(Serialize)]
struct Context {
    count: u32,
    remaining_time: String,
    remaining_time_abs: String,
    project: String,
    description: String,
    project_or_description: String,
    task: String,
}

lazy_static! {
    static ref POMODORO_STATE: RwLock<PomodoroState> = RwLock::new(Default::default());
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

fn update(toggl: &Toggl, notifiers: &Vec<Box<dyn Notifier>>) -> Result<(), Error> {
    let config = CONFIG.read().unwrap();
    let pomodoro_config = &config.pomodoro;
    let mut entries = toggl.time_entries()?;
    let mut state = POMODORO_STATE.write().unwrap();
    let mut history: Vec<(PomodoroMode, i64)> = Vec::new();

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
                    && latest_entry.project_id == x.project_id
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
                if d >= (pomodoro_config.long_break_min as i64 * 60) {
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
                    pomodoro_config.long_break_min as i64 * 60
                } else {
                    pomodoro_config.short_break_min as i64 * 60
                }
            } else {
                pomodoro_config.pomodoro_min as i64 * 60
            }
        };
        if let Some(v) = history.first() {
            if v.0 == mode_of_entry(&latest_entry) {
                duration -= v.1;
            }
        }
        state.description = latest_entry.description.clone();
        state.project = latest_entry.project_name.clone().unwrap_or_default();
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
            let (next, min) = {
                if mode_of_entry(&latest_entry) == PomodoroMode::Break {
                    (PomodoroMode::Work, pomodoro_config.pomodoro_min)
                } else {
                    (
                        PomodoroMode::Break,
                        if state.npomodoros >= pomodoro_config.long_break_after {
                            pomodoro_config.long_break_min
                        } else {
                            pomodoro_config.short_break_min
                        },
                    )
                }
            };

            if (state.nnotifications == 0)
                || (state.nnotifications == 1 && dur_secs < -300)
                || (state.nnotifications == 2 && dur_secs < -1800)
            {
                for n in notifiers {
                    n.notify(next, min)?;
                }
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
                    for n in notifiers {
                        n.notify(PomodoroMode::Work, duration.num_minutes() as u32)?;
                    }
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
    let config = CONFIG.read().unwrap();

    let interval = time::Duration::from_secs(3);
    let toggl = Toggl::new(config.toggl_token.to_string());
    let mut notifiers: Vec<Box<dyn Notifier>> = Vec::new();
    if config.notification.dbus {
        notifiers.push(Box::new(DBusNotifier::new().unwrap()));
    }
    if let Some(url) = config.notification.slack.as_ref() {
        notifiers.push(Box::new(SlackNotifier::new(url).unwrap()));
    }
    if let Some(to) = config.notification.mail.as_ref() {
        notifiers.push(Box::new(
            MailNotifier::new("toggdoro@localhost", to).unwrap(),
        ));
    }
    loop {
        if let Err(e) = update(&toggl, &notifiers) {
            println!("{}", e);
        }
        thread::sleep(interval);
    }
}

fn handle_connection(mut stream: UnixStream, templates: &Handlebars) -> Result<(), Error> {
    let config = CONFIG.read().unwrap();

    let state = POMODORO_STATE.read().unwrap();
    match state.mode {
        PomodoroMode::Idle => writeln!(stream, "{}", &config.format.idle)?,
        mode => {
            let mut context = Context {
                count: state.npomodoros,
                description: state.description.clone(),
                project: state.project.clone(),
                project_or_description: if state.project != "" {
                    state.project.clone()
                } else {
                    state.description.clone()
                },
                remaining_time: "".to_string(),
                remaining_time_abs: "".to_string(),
                task: "".to_string(),
            };

            let now = Local::now();
            if let Some(finish_time) = state.task_finish_time {
                let duration = finish_time - now;
                let timeover = duration.num_seconds() < 0;
                let template = if timeover {
                    format!("over{:?}Task", mode)
                } else {
                    format!("{:?}Task", mode)
                };
                let mins = duration.num_minutes();
                let secs = duration.num_seconds().abs() % 60;

                context.remaining_time = format!("{:02}:{:02}", mins, secs);
                context.remaining_time_abs = format!("{:02}:{:02}", mins.abs(), secs);
                context.task = templates.render(&template, &context)?;

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

            context.remaining_time = format!("{:02}:{:02}", mins, secs);
            context.remaining_time_abs = format!("{:02}:{:02}", mins.abs(), secs);

            writeln!(stream, "{}", templates.render(&template, &context)?)?;
        }
    };

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

    Config::load(&config_path)?;

    let path = env::var("XDG_RUNTIME_DIR")
        .map(|x| x.to_string() + "/toggdoro.sock")
        .unwrap_or(home.to_string() + "/.toggdoro.sock");

    let listener = UnixListener::bind(&path)?;

    let signals = Signals::new(&[SIGTERM, SIGINT])?;
    thread::spawn(move || {
        for _sig in signals.forever() {
            fs::remove_file(&path).unwrap();
            process::exit(130);
        }
    });

    thread::spawn(|| monitor());

    let templates = Arc::new({
        let mut t = Handlebars::new();
        let config = CONFIG.read().unwrap();

        t.register_template_string("Work", &config.format.work)?;
        t.register_template_string("Break", &config.format.r#break)?;
        t.register_template_string("overWork", &config.format.overwork)?;
        t.register_template_string("overBreak", &config.format.overbreak)?;
        t.register_template_string("WorkTask", &config.format.task_work)?;
        t.register_template_string("BreakTask", &config.format.task_break)?;
        t.register_template_string("overWorkTask", &config.format.task_overwork)?;
        t.register_template_string("overBreakTask", &config.format.task_overbreak)?;
        t
    });

    for stream in listener.incoming() {
        let templates = templates.clone();
        match stream {
            Ok(stream) => {
                thread::spawn(move || handle_connection(stream, &templates));
            }
            Err(err) => {
                println!("accept failed: {:?}", err);
            }
        }
    }

    Ok(())
}
