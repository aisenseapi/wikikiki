//! Operator-facing configuration (DESIGN.md §4).
//!
//! Order of precedence: env vars > wikikiki.toml > built-in defaults.
//! Every field has a default so an empty config still boots.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub paths: Paths,
    pub server: Server,
    pub ui: Ui,
    pub auth: Auth,
    pub git: Git,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            paths: Paths::default(),
            server: Server::default(),
            ui: Ui::default(),
            auth: Auth::default(),
            git: Git::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Paths {
    pub content_root: PathBuf,
    pub db_path: PathBuf,
    pub git_repo: PathBuf,
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            content_root: PathBuf::from("./content"),
            db_path: PathBuf::from("./wikikiki.db"),
            git_repo: PathBuf::from("./content"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Server {
    pub bind: String,
    pub public_url: String,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8090".to_string(),
            public_url: "http://localhost:8090".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Ui {
    pub markdown_renderer: String,
    pub allow_anonymous_read: bool,
    pub require_auth_for_write: bool,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            markdown_renderer: "comrak".to_string(),
            allow_anonymous_read: true,
            require_auth_for_write: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Auth {
    /// Cookie lifetime for human sessions.
    pub session_lifetime: HumanDuration,
    /// Default expiry on issued tokens; None = no expiry.
    pub token_default_lifetime: Option<HumanDuration>,
    /// How remote addresses are written to `access_log`: 'plain' | 'hashed' | 'none'.
    pub log_remote_addr: String,
    /// Whether the session cookie carries `Secure`. `None` (the default)
    /// derives it from `server.public_url`, so an HTTPS deployment gets the
    /// flag without anyone having to remember it, and a plain-HTTP localhost
    /// instance still works. Set explicitly when terminating TLS upstream
    /// with an http:// public_url.
    pub cookie_secure: Option<bool>,
}

impl Default for Auth {
    fn default() -> Self {
        Self {
            session_lifetime: HumanDuration(Duration::from_secs(30 * 24 * 60 * 60)),
            token_default_lifetime: Some(HumanDuration(Duration::from_secs(365 * 24 * 60 * 60))),
            log_remote_addr: "hashed".to_string(),
            cookie_secure: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Git {
    pub author_template: String,
}

impl Default for Git {
    fn default() -> Self {
        Self {
            author_template: "{actor_type}:{actor_handle} <{actor_handle}@wikikiki.local>"
                .to_string(),
        }
    }
}

/// A human-readable duration like "30d", "15m", "2h", "1y".
/// Parses into a `std::time::Duration`.
#[derive(Debug, Clone, Copy)]
pub struct HumanDuration(pub Duration);

impl HumanDuration {
    pub fn as_secs(&self) -> i64 {
        self.0.as_secs() as i64
    }
}

impl Serialize for HumanDuration {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&format_duration(self.0))
    }
}

impl<'de> Deserialize<'de> for HumanDuration {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        parse_duration(&s)
            .map(HumanDuration)
            .map_err(serde::de::Error::custom)
    }
}

fn parse_duration(s: &str) -> std::result::Result<Duration, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty duration".into());
    }
    let (num_str, unit) = s.split_at(
        s.find(|c: char| !c.is_ascii_digit())
            .unwrap_or(s.len()),
    );
    let n: u64 = num_str
        .parse()
        .map_err(|_| format!("invalid number in duration: {s}"))?;
    let mult: u64 = match unit {
        "s" | "" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        "w" => 7 * 86_400,
        "y" => 365 * 86_400,
        other => return Err(format!("unknown duration unit: '{other}'")),
    };
    Ok(Duration::from_secs(n * mult))
}

fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s % (365 * 86_400) == 0 {
        format!("{}y", s / (365 * 86_400))
    } else if s % 86_400 == 0 {
        format!("{}d", s / 86_400)
    } else if s % 3600 == 0 {
        format!("{}h", s / 3600)
    } else if s % 60 == 0 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

impl Config {
    /// Whether the session cookie should carry `Secure`.
    ///
    /// Defaults to "yes when we tell the world we are HTTPS". Without this the
    /// cookie is sent in the clear on any plain-HTTP request to the same host,
    /// which is exactly the case `Secure` exists to prevent.
    pub fn cookie_secure(&self) -> bool {
        self.auth
            .cookie_secure
            .unwrap_or_else(|| self.server.public_url.starts_with("https://"))
    }

    /// Load from a TOML path. Missing file → defaults.
    pub fn load(path: Option<&std::path::Path>) -> Result<Self> {
        let base = if let Some(p) = path {
            if p.exists() {
                let raw = std::fs::read_to_string(p)?;
                toml::from_str::<Config>(&raw)
                    .map_err(|e| AppError::Config(format!("parse {}: {e}", p.display())))?
            } else {
                Config::default()
            }
        } else {
            // Look for ./wikikiki.toml; fall back to defaults.
            let candidate = PathBuf::from("./wikikiki.toml");
            if candidate.exists() {
                let raw = std::fs::read_to_string(&candidate)?;
                toml::from_str::<Config>(&raw)
                    .map_err(|e| AppError::Config(format!("parse wikikiki.toml: {e}")))?
            } else {
                Config::default()
            }
        };
        Ok(apply_env_overrides(base))
    }
}

fn apply_env_overrides(mut cfg: Config) -> Config {
    if let Ok(v) = std::env::var("WIKIKIKI_BIND") {
        cfg.server.bind = v;
    }
    if let Ok(v) = std::env::var("WIKIKIKI_PUBLIC_URL") {
        cfg.server.public_url = v;
    }
    if let Ok(v) = std::env::var("WIKIKIKI_CONTENT_ROOT") {
        cfg.paths.content_root = PathBuf::from(v);
    }
    if let Ok(v) = std::env::var("WIKIKIKI_DB_PATH") {
        cfg.paths.db_path = PathBuf::from(v);
    }
    if let Ok(v) = std::env::var("WIKIKIKI_GIT_REPO") {
        cfg.paths.git_repo = PathBuf::from(v);
    }
    if let Ok(v) = std::env::var("WIKIKIKI_ALLOW_ANON_READ") {
        cfg.ui.allow_anonymous_read = matches!(v.as_str(), "1" | "true" | "yes");
    }
    if let Ok(v) = std::env::var("WIKIKIKI_REQUIRE_AUTH_WRITE") {
        cfg.ui.require_auth_for_write = matches!(v.as_str(), "1" | "true" | "yes");
    }
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_roundtrip() {
        let cases = ["30d", "15m", "2h", "1y", "45s"];
        for c in cases {
            let d = parse_duration(c).unwrap();
            let s = format_duration(d);
            assert_eq!(s, c, "roundtrip {c}");
        }
    }

    #[test]
    fn cookie_secure_follows_public_url_unless_overridden() {
        let mut cfg = Config::default();
        assert!(!cfg.cookie_secure(), "http://localhost default");

        cfg.server.public_url = "https://wiki.example.org".into();
        assert!(cfg.cookie_secure(), "https public_url");

        // Explicit override wins both ways — e.g. TLS terminated upstream.
        cfg.auth.cookie_secure = Some(false);
        assert!(!cfg.cookie_secure());
        cfg.server.public_url = "http://localhost:8090".into();
        cfg.auth.cookie_secure = Some(true);
        assert!(cfg.cookie_secure());
    }

    #[test]
    fn defaults_load() {
        let cfg = Config::default();
        assert!(cfg.ui.allow_anonymous_read);
        assert!(cfg.ui.require_auth_for_write);
        assert_eq!(cfg.server.bind, "127.0.0.1:8090");
    }

    #[test]
    fn parse_minimal_toml() {
        let raw = r#"
            [server]
            bind = "0.0.0.0:9090"
        "#;
        let cfg: Config = toml::from_str(raw).unwrap();
        assert_eq!(cfg.server.bind, "0.0.0.0:9090");
        // Other fields stay at defaults.
        assert_eq!(cfg.paths.content_root, PathBuf::from("./content"));
    }
}
