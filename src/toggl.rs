use chrono::{DateTime, Local};
use failure::Error;
use serde_derive::{Deserialize, Serialize};

pub struct Toggl {
    token: String,
    client: reqwest::Client,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TimeEntry {
    pub at: DateTime<Local>,
    pub billable: bool,
    pub client_name: Option<String>,
    #[serde(default)]
    pub description: String,
    pub duration: i64,
    pub duronly: bool,
    pub id: u64,
    pub permissions: Option<Vec<String>>,
    pub project_active: Option<bool>,
    pub project_color: Option<String>,
    pub project_id: Option<u64>,
    pub project_name: Option<String>,
    pub server_deleted_at: Option<DateTime<Local>>,
    //pub started_with,
    pub start: DateTime<Local>,
    pub stop: Option<DateTime<Local>>,
    pub tag_ids: Vec<u64>,
    pub tags: Vec<String>,
    pub task_id: Option<u64>,
    pub task_name: Option<String>,
    pub user_id: u64,
    pub workspace_id: u64,
}

#[derive(Debug, Deserialize)]
pub struct Data<T> {
    pub data: T,
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
            .get("https://api.track.toggl.com/api/v9/me/time_entries")
            .basic_auth(&self.token, Some("api_token"))
            .send()?;
        let entries = res.json::<Vec<TimeEntry>>()?;
        Ok(entries)
    }
}
