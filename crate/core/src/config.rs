use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Top-level TOML configuration.
///
/// `keepalive` and `retry` live at the top level because every worker shares
/// the same connection-lifecycle policy. Per-connection copies would let
/// drift creep in and bloat the TOML.
#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub keepalive: KeepaliveConfig,
    #[serde(default)]
    pub retry: RetryConfig,
    #[serde(default)]
    pub connections: Vec<ConnectionConfig>,
}

/// One OpenSSH process connected to one host. A connection owns its forwards
/// and SSH flags; keepalive/retry come from the parent [`Config`].
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConnectionConfig {
    /// Unique connection identifier.
    pub name: String,
    /// Optional human-readable note displayed by the GUI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// SSH destination (`user@host` or Host alias). Defaults to `name` for compatibility.
    pub host: Option<String>,
    /// Optional SSH user. When set and `host` does not already contain `@`,
    /// the effective destination becomes `user@host`. Empty/`None` leaves
    /// `host`/`name` unchanged so existing `user@host` configs keep working.
    #[serde(default)]
    pub user: Option<String>,
    /// Optional password for non-interactive password auth via `sshpass -e`.
    /// When set, `BatchMode=yes` is dropped so ssh attempts password auth as
    /// a fallback to publickey. Stored in plaintext — protect the config file.
    #[serde(default)]
    pub password: Option<String>,
    /// Optional SSH server port. Omitting this uses OpenSSH's default (22).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    /// An optional explicit path to ssh (ssh.exe on Windows).
    pub ssh_path: Option<PathBuf>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    pub forwards: Vec<ForwardConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ForwardConfig {
    /// Whether this forward is passed to ssh. Missing values default to enabled
    /// so existing configuration files preserve their behaviour.
    #[serde(default = "enabled_by_default", skip_serializing_if = "is_enabled")]
    pub enabled: bool,
    /// `remote` → `ssh -R`; `local` → `ssh -L`; `dynamic` → `ssh -D` (SOCKS proxy).
    pub mode: ForwardMode,
    /// For `local` / `remote`: full `[bind:]listen:target_host:target_port`, or shorthand
    /// `listen` when target is `127.0.0.1:listen` (e.g. `5173` or `5173:127.0.0.1:5173`).
    /// For `dynamic`: `[bind:]port` (e.g. `1080` or `0.0.0.0:1080`).
    pub forward: String,
    /// Optional human-readable note displayed by the GUI (e.g. "home SSH",
    /// "SOCKS proxy for Chrome"). Empty values fall back to `None` so the
    /// TOML stays free of `description = ""` clutter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ForwardMode {
    Local,
    Remote,
    /// `ssh -D` — local SOCKS proxy listening on `[bind:]port`.
    Dynamic,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KeepaliveConfig {
    #[serde(default = "default_keepalive_interval")]
    pub interval: u64,
    #[serde(default = "default_keepalive_count")]
    pub count_max: u32,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RetryConfig {
    #[serde(default = "default_retry_initial")]
    pub initial_seconds: u64,
    #[serde(default = "default_retry_maximum")]
    pub maximum_seconds: u64,
    #[serde(default = "default_retry_stable")]
    pub stable_seconds: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LogConfig {
    pub file: Option<PathBuf>,
}

fn enabled_by_default() -> bool {
    true
}

fn is_enabled(enabled: &bool) -> bool {
    *enabled
}
fn default_keepalive_interval() -> u64 {
    30
}
fn default_keepalive_count() -> u32 {
    3
}
fn default_connect_timeout() -> u64 {
    15
}
fn default_retry_initial() -> u64 {
    1
}
fn default_retry_maximum() -> u64 {
    60
}
fn default_retry_stable() -> u64 {
    60
}

/// `ssh -L` / `-R` spec: `listen` (same port on `127.0.0.1`), `port:host:port`, or
/// `[bind:]listen:host:port`.
fn is_valid_static_forward_spec(spec: &str) -> bool {
    let parts = split_forward_spec(spec);
    match parts.as_slice() {
        [port] => is_valid_port(port),
        [listen_port, _target_host, target_port] => {
            is_valid_port(listen_port) && is_valid_port(target_port)
        }
        [listen_host, listen_port, _target_host, target_port] => {
            let _ = listen_host;
            is_valid_port(listen_port) && is_valid_port(target_port)
        }
        _ => false,
    }
}

/// Expands L/R shorthand `port` → `port:127.0.0.1:port` for `ssh -L`/`-R`.
pub(crate) fn canonicalize_static_forward_spec(spec: &str) -> String {
    let trimmed = spec.trim();
    let parts = split_forward_spec(trimmed);
    if parts.len() == 1 && is_valid_port(parts[0]) {
        return format!("{trimmed}:127.0.0.1:{trimmed}");
    }
    trimmed.to_string()
}

/// `ssh -D` spec: `[bind_host:]port` where the trailing segment is the only required
/// one and must be a valid TCP port. IPv6 binds keep their `[…]` brackets.
fn is_valid_dynamic_forward_spec(spec: &str) -> bool {
    let parts = split_forward_spec(spec);
    let port = match parts.as_slice() {
        [port] => port,
        [_host, port] => port,
        _ => return false,
    };
    is_valid_port(port)
}

fn is_valid_port(text: &str) -> bool {
    text.parse::<u16>().is_ok()
}

/// Split a forward spec on `:` while respecting IPv6 brackets so that
/// `[::1]:1080` parses as `["[::1]", "1080"]` rather than four junk segments.
fn split_forward_spec(spec: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut in_brackets = false;
    for (idx, ch) in spec.char_indices() {
        match ch {
            '[' => in_brackets = true,
            ']' => in_brackets = false,
            ':' if !in_brackets => {
                parts.push(&spec[start..idx]);
                start = idx + 1;
            }
            _ => {}
        }
    }
    parts.push(&spec[start..]);
    parts
}
impl Default for KeepaliveConfig {
    fn default() -> Self {
        Self {
            interval: default_keepalive_interval(),
            count_max: default_keepalive_count(),
            connect_timeout: default_connect_timeout(),
        }
    }
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            initial_seconds: default_retry_initial(),
            maximum_seconds: default_retry_maximum(),
            stable_seconds: default_retry_stable(),
        }
    }
}

impl ConnectionConfig {
    /// Effective SSH destination passed on the ssh command line:
    /// `user@host` when a separate `user` is configured and `host` itself
    /// does not already embed `user@`, otherwise `host` (or `name` as fallback).
    /// Returns an owned `String` because the `user@` prefix may need to be
    /// synthesised and cannot borrow from a single field.
    pub fn destination(&self) -> String {
        let base = self.host.as_deref().unwrap_or(&self.name);
        match self.user.as_deref() {
            Some(user) if !user.trim().is_empty() && !base.contains('@') => {
                format!("{user}@{base}")
            }
            _ => base.to_string(),
        }
    }

    /// True when a non-empty `password` is configured and should trigger the
    /// `sshpass`-based password-auth fallback (and suppress `BatchMode=yes`).
    pub fn has_password(&self) -> bool {
        self.password.as_deref().is_some_and(|p| !p.is_empty())
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("cannot read configuration {}", path.display()))?;
        let config: Self =
            toml::from_str(&text).with_context(|| format!("invalid TOML in {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    /// Serialises to TOML and writes atomically (write to temp, rename) so that a partial
    /// edit can never leave an unreadable configuration on disk.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let body = toml::to_string_pretty(self)
            .with_context(|| format!("cannot serialise configuration for {}", path.display()))?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
        let temp = path.with_extension("toml.tmp");
        fs::write(&temp, body).with_context(|| format!("cannot write {}", temp.display()))?;
        fs::rename(&temp, path)
            .with_context(|| format!("cannot rename {} to {}", temp.display(), path.display()))?;
        Ok(())
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.keepalive.interval == 0
            || self.keepalive.count_max == 0
            || self.keepalive.connect_timeout == 0
        {
            bail!("keepalive values must be greater than zero");
        }
        if self.retry.initial_seconds == 0
            || self.retry.maximum_seconds < self.retry.initial_seconds
            || self.retry.stable_seconds == 0
        {
            bail!(
                "retry settings are invalid: initial must be > 0 and <= maximum, stable must be > 0"
            );
        }
        let mut names = HashSet::new();
        for connection in &self.connections {
            if connection.name.trim().is_empty() {
                bail!("connection name must not be empty");
            }
            if connection.destination().trim().is_empty() {
                bail!("connection {} has an empty host", connection.name);
            }
            if connection.port == Some(0) {
                bail!("connection {}: port must be 1-65535", connection.name);
            }
            if connection.forwards.is_empty() {
                bail!("connection {} has no forwards", connection.name);
            }
            for forward in &connection.forwards {
                let spec = forward.forward.trim();
                if spec.is_empty() {
                    bail!("connection {} has an empty forward", connection.name);
                }
                match forward.mode {
                    ForwardMode::Local | ForwardMode::Remote => {
                        if !is_valid_static_forward_spec(spec) {
                            bail!(
                                "connection {}: forward {:?} for mode {:?} must be `listen`, `listen:target_host:target_port`, or `[bind:]listen:target_host:target_port`",
                                connection.name,
                                forward.forward,
                                forward.mode,
                            );
                        }
                    }
                    ForwardMode::Dynamic => {
                        if !is_valid_dynamic_forward_spec(spec) {
                            bail!(
                                "connection {}: forward {:?} for dynamic mode must be `[bind:]port` (1-65535)",
                                connection.name,
                                forward.forward,
                            );
                        }
                    }
                }
            }
            if !names.insert(&connection.name) {
                bail!("duplicate connection name: {}", connection.name);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/unit/config.rs"]
mod tests;
