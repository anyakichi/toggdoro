use failure::Error;

use crate::pomodoro::PomodoroMode;

pub mod dbus;
pub mod mail;
pub mod slack;

pub trait Notifier {
    fn notify(&self, mode: PomodoroMode, min: u32) -> Result<(), Error>;
}
