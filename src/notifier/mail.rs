use failure::Error;
use lettre::{SendmailTransport, Transport};
use lettre_email::Email;

use crate::notifier::Notifier;
use crate::pomodoro::PomodoroMode;

pub struct MailNotifier {
    from: String,
    to: String,
}

impl MailNotifier {
    pub fn new(from: &str, to: &str) -> Result<Self, Error> {
        Ok(MailNotifier {
            from: from.to_string(),
            to: to.to_string(),
        })
    }
}

impl Notifier for MailNotifier {
    fn notify(&self, mode: PomodoroMode, min: u32) -> Result<(), Error> {
        let email = Email::builder()
            .from(&self.from as &str)
            .to(&self.to as &str)
            .subject(format!("{:?} {} min", mode, min))
            .text("")
            .build()?;

        let mut mailer = SendmailTransport::new();
        mailer.send(email.into())?;
        Ok(())
    }
}
