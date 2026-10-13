use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const DEFAULT_PATH: &str = "sixb6t-config.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub servers: Vec<String>,
    pub username: String,
    pub password: String,
    /// Always send /register right after joining
    pub force_register: bool,
    /// Never send /register
    pub no_register: bool,
    /// Seconds between /login and walking
    pub walk_delay: f64,
    /// Seconds before a walk phase is aborted
    pub walk_timeout: f64,
    /// Stay connected after finishing
    pub stay: bool,
    pub trapdoor: Option<[i32; 3]>,
    pub trigger_user: String,
    pub trigger_word: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            servers: ["alt.6b6t.org", "alt4.6b6t.org", "alt5.6b6t.org", "alt2.6b6t.org", "alt3.6b6t.org"].iter().map(|s| (*s).to_owned()).collect(),
            username: String::new(),
            password: String::new(),
            force_register: false,
            no_register: false,
            walk_delay: 3.0,
            walk_timeout: 30.0,
            stay: false,
            trapdoor: None,
            trigger_user: String::new(),
            trigger_word: String::new(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(path).with_context(|| format!("Reading {}", path.display()))?;
        Ok(Some(serde_json::from_str(&text).with_context(|| format!("Parsing {}", path.display()))?))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(self)?).with_context(|| format!("Writing {}", path.display()))
    }

    pub fn is_complete(&self) -> bool {
        !self.server_list().is_empty() && !self.username.is_empty() && !self.password.is_empty()
    }

    pub fn server_list(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for s in &self.servers {
            let s = s.trim();
            if !s.is_empty() && !out.iter().any(|o| o == s) {
                out.push(s.to_owned());
            }
        }
        out
    }

    pub fn trigger_text(&self) -> String {
        if self.trigger_user.trim().is_empty() || self.trigger_word.trim().is_empty() {
            "(off)".to_owned()
        } else {
            format!("when {} says \"{}\"", self.trigger_user.trim(), self.trigger_word.trim())
        }
    }

    pub fn register_mode(&self) -> &'static str {
        if self.no_register {
            "never (login only)"
        } else if self.force_register {
            "always, right after joining"
        } else {
            "auto (when the server asks)"
        }
    }
}