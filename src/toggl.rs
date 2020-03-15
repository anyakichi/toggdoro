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
