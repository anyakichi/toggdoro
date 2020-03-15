use failure::Error;
use slack_hook::{PayloadBuilder, Slack};

use crate::notifier::Notifier;
use crate::pomodoro::PomodoroMode;

pub struct SlackNotifier {
    slack: Slack,
}

impl SlackNotifier {
    pub fn new(url: &str) -> Result<Self, Error> {
        let slack = Slack::new(url as &str).map_err(|e| format_err!("{}", e))?;
        Ok(SlackNotifier { slack })
    }
}

impl Notifier for SlackNotifier {
    fn notify(&self, mode: PomodoroMode, min: u32) -> Result<(), Error> {
        let emoji = if mode == PomodoroMode::Work {
            ":tomato:"
        } else {
            ":coffee:"
        };
        let p = PayloadBuilder::new()
            .username("toggdoro")
            .icon_emoji(emoji)
            .text(format!("{:?} {} min", mode, min))
            .build()
            .map_err(|e| format_err!("{}", e))?;

        self.slack.send(&p).map_err(|e| format_err!("{}", e))?;
        Ok(())
    }
}
