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
    #[serde(default)]
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

impl Toggl {
    pub fn new(token: String) -> Self {
        Toggl {
            token,
            client: reqwest::Client::new(),
        }
    }

    pub fn time_entries(&self) -> Result<Vec<TimeEntry>, Error> {
        let mut res = self
            .client
            .get("https://www.toggl.com/api/v8/time_entries")
            .basic_auth(&self.token, Some("api_token"))
            .send()?;
        let entries = res.json::<Vec<TimeEntry>>()?;
        Ok(entries)
    }
}
