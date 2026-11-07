//! Runtime paths and user config. All state lives in `~/.insta` (or
//! `INSTA_HOME`), never in the repo.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

/// 7777-7779 belong to the sibling daemons (Telegram, WhatsApp, Slack), so
/// all four can run side by side.
pub const DEFAULT_PORT: u16 = 7780;

#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
    pub db: PathBuf,
    pub config: PathBuf,
    pub style: PathBuf,
}

impl Paths {
    pub fn resolve() -> Result<Paths> {
        let home = match std::env::var_os("INSTA_HOME") {
            Some(h) if !h.is_empty() => PathBuf::from(h),
            _ => {
                let user_home = std::env::var_os("HOME").context("HOME is not set")?;
                PathBuf::from(user_home).join(".insta")
            }
        };
        std::fs::create_dir_all(&home).with_context(|| format!("creating {}", home.display()))?;
        // Holds the DB with names, triage state and voice profiles: owner-only.
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
        Ok(Paths { db: home.join("insta.db"), config: home.join("config.json"), style: home.join("style.md"), home })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Config { port: DEFAULT_PORT }
    }
}

impl Config {
    pub fn load(paths: &Paths) -> Result<Config> {
        match std::fs::read_to_string(&paths.config) {
            Ok(text) => serde_json::from_str(&text).with_context(|| format!("parsing {}", paths.config.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Env vars are process-global, so every test that touches INSTA_HOME
    // goes through this one test.
    #[test]
    fn paths_respect_insta_home_and_config_defaults_port_7780() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("state");
        unsafe { std::env::set_var("INSTA_HOME", &home) };
        let p = Paths::resolve().unwrap();
        assert_eq!(p.db, home.join("insta.db"));
        assert_eq!(p.style, home.join("style.md"));
        assert_eq!(p.config, home.join("config.json"));
        let mode = std::fs::metadata(&home).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);

        assert_eq!(Config::load(&p).unwrap().port, 7780);
        std::fs::write(&p.config, r#"{"port": 8000}"#).unwrap();
        assert_eq!(Config::load(&p).unwrap().port, 8000);
        std::fs::write(&p.config, "{not json").unwrap();
        assert!(Config::load(&p).is_err());
        unsafe { std::env::remove_var("INSTA_HOME") };
    }
}
