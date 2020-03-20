use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Local};
use failure::Error;
use lazy_static::lazy_static;
use serde_derive::{Deserialize, Serialize};

pub struct Toggl {
    token: String,
    client: reqwest::Client,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Project {
    pub id: u64,
    pub wid: u64,
    pub cid: Option<u64>,
    pub name: String,
    pub billable: bool,
    pub is_private: bool,
    pub active: bool,
    pub at: DateTime<Local>,
    pub template: bool,
    pub color: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TimeEntry {
    pub id: u64,
    pub guid: String,
    pub wid: u64,
    pub pid: Option<u64>,
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
    pub uid: u64,
}

#[derive(Debug, Deserialize)]
pub struct Data<T> {
    pub data: T,
}

lazy_static! {
    pub static ref PROJECTS: RwLock<HashMap<u64, Arc<Project>>> = RwLock::new(HashMap::new());
}

impl Toggl {
    pub fn new(token: String) -> Self {
        Toggl {
            token,
            client: reqwest::Client::new(),
        }
    }

    pub fn project(&self, pid: u64) -> Option<Arc<Project>> {
        let mut projects = PROJECTS.write().unwrap();
        match projects.get(&pid) {
            Some(project) => Some(project.clone()),
            None => self
                .projects(pid)
                .map(|project| Arc::new(project))
                .and_then(|p| {
                    projects.insert(pid, p.clone());
                    Ok(p)
                })
                .ok(),
        }
    }

    pub fn projects(&self, pid: u64) -> Result<Project, Error> {
        let mut res = self
            .client
            .get(&format!("https://www.toggl.com/api/v8/projects/{}", pid))
            .basic_auth(&self.token, Some("api_token"))
            .send()?;
        let project_data = res.json::<Data<Project>>()?;
        Ok(project_data.data)
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
