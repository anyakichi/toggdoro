use std::fs::File;
use std::io::{BufReader, Read};
use std::sync::RwLock;

use failure::Error;

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    pub version: u8,
    pub toggl_token: String,
    pub socket: Option<String>,

    #[serde(default)]
    pub notification: NotificationConfig,

    #[serde(default)]
    pub pomodoro: PomodoroConfig,

    #[serde(default)]
    pub format: FormatConfig,
}

impl Config {
    pub fn load(path: &str) -> Result<(), Error> {
        let mut c = CONFIG.write().unwrap();

        let file = File::open(path)?;
        let mut buf_reader = BufReader::new(file);
        let mut contents = String::new();
        buf_reader.read_to_string(&mut contents)?;
        let config = toml::from_str(&contents)?;
        *c = config;

        Ok(())
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct NotificationConfig {
    #[serde(default)]
    pub dbus: bool,

    pub mail: Option<String>,

    pub slack: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PomodoroConfig {
    #[serde(default = "default_pomodoro_min")]
    pub pomodoro_min: u32,

    #[serde(default = "default_short_break_min")]
    pub short_break_min: u32,

    #[serde(default = "default_long_break_min")]
    pub long_break_min: u32,

    #[serde(default = "default_long_break_after")]
    pub long_break_after: u32,
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

#[derive(Debug, Deserialize)]
pub struct FormatConfig {
    #[serde(default = "default_format_idle")]
    pub idle: String,

    #[serde(default = "default_format_work")]
    pub work: String,

    #[serde(default = "default_format_break")]
    pub r#break: String,

    #[serde(default = "default_format_work")]
    pub overwork: String,

    #[serde(default = "default_format_break")]
    pub overbreak: String,

    #[serde(default = "default_format_task")]
    pub task_work: String,

    #[serde(default = "default_format_task")]
    pub task_break: String,

    #[serde(default = "default_format_task")]
    pub task_overwork: String,

    #[serde(default = "default_format_task")]
    pub task_overbreak: String,
}

impl Default for FormatConfig {
    fn default() -> Self {
        Self {
            idle: default_format_idle(),
            work: default_format_work(),
            r#break: default_format_break(),
            overwork: default_format_work(),
            overbreak: default_format_break(),
            task_work: default_format_task(),
            task_break: default_format_task(),
            task_overwork: default_format_task(),
            task_overbreak: default_format_task(),
        }
    }
}

fn default_format_idle() -> String {
    "idle".to_string()
}

fn default_format_work() -> String {
    "Work {{count}}[{{remaining_time}}{{task}}]".to_string()
}

fn default_format_break() -> String {
    "Break {{count}}[{{remaining_time}}{{task}}]".to_string()
}

fn default_format_task() -> String {
    "|{{remaining_time}}".to_string()
}

lazy_static! {
    pub static ref CONFIG: RwLock<Config> = RwLock::new(Default::default());
}
