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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trapdoor: Option<[i32; 3]>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub trigger_word: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trapdoor2: Option<[i32; 3]>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub trigger_word2: String,
    pub trigger_user: String,
    pub traps: Vec<Trap>,
    pub auto_totem: bool,
    pub auto_eat: bool,
    pub kill_aura: bool,
    pub eat_below: u32,
    pub kill_aura_range: f64,
    pub kill_aura_hostiles: bool,
    pub kill_aura_neutral: bool,
    pub kill_aura_players: bool,
    pub kill_aura_never: Vec<String>,
    pub kill_aura_always: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Trap {
    pub pos: [i32; 3],
    pub word: String,
}

pub fn normalize_name(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase()
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
            trigger_word: String::new(),
            trapdoor2: None,
            trigger_word2: String::new(),
            trigger_user: String::new(),
            traps: Vec::new(),
            auto_totem: true,
            auto_eat: true,
            kill_aura: true,
            eat_below: 14,
            kill_aura_range: 3.0,
            kill_aura_hostiles: true,
            kill_aura_neutral: false,
            kill_aura_players: false,
            kill_aura_never: Vec::new(),
            kill_aura_always: Vec::new(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(path).with_context(|| format!("Reading {}", path.display()))?;
        let mut cfg: Self = serde_json::from_str(&text).with_context(|| format!("Parsing {}", path.display()))?;
        cfg.normalize();
        Ok(Some(cfg))
    }

    pub fn normalize(&mut self) {
        if let Some(pos) = self.trapdoor.take() {
            let word = std::mem::take(&mut self.trigger_word);
            self.traps.push(Trap { pos, word });
        }
        if let Some(pos) = self.trapdoor2.take() {
            let word = std::mem::take(&mut self.trigger_word2);
            self.traps.push(Trap { pos, word });
        }
        self.trigger_word.clear();
        self.trigger_word2.clear();
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
        let user = self.trigger_user.trim();
        let words: Vec<String> = self
            .traps
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.word.trim().is_empty())
            .map(|(i, t)| format!("\"{}\" -> trapdoor {}", t.word.trim(), i + 1))
            .collect();
        if user.is_empty() || words.is_empty() {
            "(off)".to_owned()
        } else {
            format!("when {} says {}", user, words.join(" or "))
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