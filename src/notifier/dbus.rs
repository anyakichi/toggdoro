use failure::{format_err, Error};

use crate::notifier::Notifier;
use crate::pomodoro::PomodoroMode;

pub struct DBusNotifier;

impl DBusNotifier {
    pub fn new() -> Result<Self, Error> {
        Ok(DBusNotifier)
    }
}

impl Notifier for DBusNotifier {
    fn notify(&self, mode: PomodoroMode, min: u32) -> Result<(), Error> {
        notify_rust::Notification::new()
            .summary("Toggdoro")
            .body(&format!("{:?} {} min", mode, min))
            .show()
            .map_err(|e| format_err!("{}", e))?;
        Ok(())
    }
}
