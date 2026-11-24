use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::state_file;
use crate::providers::Candidate;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnboardingStatus {
    #[default]
    Unknown,
    Started,
    Done,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct State {
    pub choices: HashMap<String, Candidate>,
    pub offsets: HashMap<String, i64>,
    pub onboarding: OnboardingStatus,
}

impl State {
    pub fn path() -> PathBuf {
        state_file()
    }

    pub fn load() -> Self {
        fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, text);
        }
    }

    pub fn offset_for(&self, key: &str) -> i64 {
        self.offsets.get(key).copied().unwrap_or(0)
    }

    pub fn set_offset(&mut self, key: &str, ms: i64) {
        if ms == 0 {
            self.offsets.remove(key);
        } else {
            self.offsets.insert(key.to_string(), ms);
        }
    }
}
