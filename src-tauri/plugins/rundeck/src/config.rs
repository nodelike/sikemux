// ~/.rd-config is shared with the bash `rnd` CLI so terminal and in-app logins
// use one credential store. We read the CLI's `%q`-quoted values, and on save
// rewrite only the keys this plugin owns, keeping every other line as it was.
//
// One global RwLock<RundeckConfig> caches the file; it is re-read whenever the
// file's mtime changes, so a `rnd login` in a terminal is picked up.

use std::fs;
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::{RundeckError, RundeckResult};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RundeckConfig {
    pub url: String,
    pub user: String,
    pub token: String,
    /// Id of the token this app minted, so logout can revoke it. Empty for a
    /// pasted token or one the CLI wrote.
    pub token_id: String,
    /// Explicit user acknowledgement for plaintext HTTP. Even when enabled,
    /// every request must resolve exclusively to private or loopback addresses.
    #[serde(default)]
    pub allow_insecure_private_http: bool,
}

impl RundeckConfig {
    pub fn is_configured(&self) -> bool {
        !self.url.is_empty() && !self.token.is_empty()
    }
}

pub fn config_path() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".rd-config"))
}

/// Strip POSIX-style %q quoting that bash's `printf '%q'` produces. Handles
/// the common subset we actually emit / receive: single-quoted strings,
/// dollar-quoted ($'...'), and plain bare words.
fn unquote(raw: &str) -> String {
    let s = raw.trim();
    if let Some(inner) = s
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return inner.replace("'\\''", "'");
    }
    if let Some(inner) = s
        .strip_prefix("$'")
        .and_then(|rest| rest.strip_suffix('\''))
    {
        // $'...': interpret \n, \t, \', \\
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some('\'') => out.push('\''),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        }
        return out;
    }
    if let Some(inner) = s.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')) {
        return inner.to_string();
    }
    s.to_string()
}

/// Quote a value for write — single-quoted, escaping embedded single quotes
/// using the bash idiom `'\''`. Round-trips through `unquote`.
fn quote(s: &str) -> String {
    let escaped = s.replace('\'', "'\\''");
    format!("'{escaped}'")
}

const OWNED_KEYS: [&str; 5] = [
    "RD_URL",
    "RD_USER",
    "RD_TOKEN",
    "RD_TOKEN_ID",
    "RD_ALLOW_INSECURE_PRIVATE_HTTP",
];

fn line_entry(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.starts_with('#') {
        return None;
    }
    line.split_once('=').map(|(key, value)| (key.trim(), value))
}

fn parse(content: &str) -> RundeckConfig {
    let mut cfg = RundeckConfig::default();
    for (key, value) in content.lines().filter_map(line_entry) {
        let val = unquote(value);
        match key {
            "RD_URL" => cfg.url = val.trim_end_matches('/').to_string(),
            "RD_USER" => cfg.user = val,
            "RD_TOKEN" => cfg.token = val,
            "RD_TOKEN_ID" => cfg.token_id = val,
            "RD_ALLOW_INSECURE_PRIVATE_HTTP" => {
                cfg.allow_insecure_private_http = matches!(val.as_str(), "1" | "true" | "yes")
            }
            _ => {}
        }
    }
    cfg
}

fn read_file() -> RundeckResult<RundeckConfig> {
    let Some(path) = config_path() else {
        return Ok(RundeckConfig::default());
    };
    match fs::read_to_string(&path) {
        Ok(content) => Ok(parse(&content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(RundeckConfig::default()),
        Err(error) => Err(error.into()),
    }
}

fn owned_line(key: &str, cfg: &RundeckConfig) -> String {
    let value = match key {
        "RD_URL" => cfg.url.as_str(),
        "RD_USER" => cfg.user.as_str(),
        "RD_TOKEN" => cfg.token.as_str(),
        "RD_TOKEN_ID" => cfg.token_id.as_str(),
        _ if cfg.allow_insecure_private_http => "1",
        _ => "0",
    };
    format!("{key}={}", quote(value))
}

/// The new file: every existing line kept, owned keys rewritten in place (or
/// appended when missing) and duplicates of them dropped.
fn render(existing: &str, cfg: &RundeckConfig) -> String {
    let mut written: Vec<&str> = Vec::new();
    let mut out = String::with_capacity(existing.len() + 256);
    for line in existing.lines() {
        let owned = line_entry(line)
            .and_then(|(key, _)| OWNED_KEYS.iter().find(|owned| **owned == key).copied());
        match owned {
            Some(key) if written.contains(&key) => continue,
            Some(key) => {
                written.push(key);
                out.push_str(&owned_line(key, cfg));
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    for key in OWNED_KEYS.iter().filter(|key| !written.contains(key)) {
        out.push_str(&owned_line(key, cfg));
        out.push('\n');
    }
    out
}

fn write_file(cfg: &RundeckConfig) -> RundeckResult<()> {
    let Some(path) = config_path() else {
        return Err(RundeckError::Api("no HOME directory".into()));
    };
    write_file_at(&path, cfg)
}

fn write_file_at(path: &Path, cfg: &RundeckConfig) -> RundeckResult<()> {
    let existing = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let parent = path
        .parent()
        .ok_or_else(|| RundeckError::Api("invalid config path".into()))?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    temp.write_all(render(&existing, cfg).as_bytes())?;
    temp.as_file_mut().sync_all()?;
    temp.persist(path).map_err(|e| RundeckError::Io(e.error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
pub fn validate_base_url(raw: &str) -> RundeckResult<()> {
    validate_base_url_with_policy(raw, false)
}

pub fn validate_base_url_with_policy(
    raw: &str,
    allow_insecure_private_http: bool,
) -> RundeckResult<()> {
    let url = url::Url::parse(raw).map_err(|_| RundeckError::BadArg("invalid Rundeck URL"))?;
    if url.username() != "" || url.password().is_some() {
        return Err(RundeckError::BadArg(
            "credentials in the Rundeck URL are not allowed",
        ));
    }
    if !matches!(url.scheme(), "http" | "https") {
        return Err(RundeckError::BadArg("Rundeck URL must use HTTP or HTTPS"));
    }
    if url.host_str().is_none() {
        return Err(RundeckError::BadArg("Rundeck URL must include a host"));
    }
    if url.scheme() == "http" && !allow_insecure_private_http {
        return Err(RundeckError::BadArg(
            "plaintext HTTP requires explicit private-network acknowledgement",
        ));
    }
    Ok(())
}

fn is_allowed_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
        IpAddr::V6(ip) => {
            ip.to_ipv4_mapped()
                .is_some_and(|mapped| is_allowed_private_ip(mapped.into()))
                || ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
        }
    }
}

#[derive(Clone, Debug)]
pub struct ValidatedTransport {
    host: String,
    private_http_addresses: Vec<std::net::SocketAddr>,
}

impl ValidatedTransport {
    pub fn pins_private_dns(&self) -> bool {
        !self.private_http_addresses.is_empty()
    }

    /// Identifies exactly what a client built from this transport is pinned to.
    pub fn pin_key(&self) -> String {
        let mut addresses: Vec<String> = self
            .private_http_addresses
            .iter()
            .map(std::net::SocketAddr::to_string)
            .collect();
        addresses.sort();
        format!("{}|{}", self.host, addresses.join(","))
    }

    pub fn pin_dns(&self, builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
        if self.private_http_addresses.is_empty() {
            builder
        } else {
            builder.resolve_to_addrs(&self.host, &self.private_http_addresses)
        }
    }
}

/// Validate transport policy immediately before sending credentials or a
/// bearer token. HTTP is supported for private Rundeck installations only
/// after explicit acknowledgement and only while DNS remains private.
pub async fn validate_transport(
    raw: &str,
    allow_insecure_private_http: bool,
) -> RundeckResult<ValidatedTransport> {
    validate_base_url_with_policy(raw, allow_insecure_private_http)?;
    let url = url::Url::parse(raw).map_err(|_| RundeckError::BadArg("invalid Rundeck URL"))?;
    let host = url
        .host_str()
        .ok_or(RundeckError::BadArg("Rundeck URL must include a host"))?
        .to_string();
    if url.scheme() == "https" {
        return Ok(ValidatedTransport {
            host,
            private_http_addresses: Vec::new(),
        });
    }
    let port = url.port_or_known_default().ok_or(RundeckError::BadArg(
        "Rundeck URL must include a valid port",
    ))?;
    let addresses: Vec<_> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|_| RundeckError::BadArg("Rundeck HTTP host could not be resolved"))?
        .collect();
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|address| !is_allowed_private_ip(address.ip()))
    {
        return Err(RundeckError::BadArg(
            "plaintext Rundeck HTTP is allowed only when every resolved address is private or loopback",
        ));
    }
    Ok(ValidatedTransport {
        host,
        private_http_addresses: addresses,
    })
}

struct CacheEntry {
    cfg: RundeckConfig,
    /// mtime of `~/.rd-config` at the last successful read. `None` means
    /// the cache is empty / the file didn't exist.
    seen_mtime: Option<SystemTime>,
}

fn cache() -> &'static RwLock<CacheEntry> {
    static C: OnceLock<RwLock<CacheEntry>> = OnceLock::new();
    C.get_or_init(|| {
        RwLock::new(CacheEntry {
            cfg: RundeckConfig::default(),
            seen_mtime: None,
        })
    })
}

fn mtime_of(path: &PathBuf) -> Option<SystemTime> {
    fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

/// Reload the in-process cache from disk when `~/.rd-config` has changed.
/// Called before every Rundeck API request — used to re-read the file
/// each time (~600 B), which was visible during heavy log-tail polling
/// (2 pollers × every 1.5s × multiple disk reads each). Now we stat the
/// file (one syscall) and only re-read on an mtime change, so a long
/// session with no `rnd login` does effectively zero disk work.
pub async fn refresh_from_disk() -> RundeckResult<RundeckConfig> {
    let path = match config_path() {
        Some(p) => p,
        None => return Ok(RundeckConfig::default()),
    };
    let cur_mtime = mtime_of(&path);
    {
        let r = cache().read().await;
        if r.seen_mtime == cur_mtime && cur_mtime.is_some() {
            return Ok(r.cfg.clone());
        }
    }
    let fresh = read_file()?;
    let mut w = cache().write().await;
    w.cfg = fresh.clone();
    w.seen_mtime = cur_mtime;
    Ok(fresh)
}

/// Re-reads the file even when its mtime looks unchanged.
pub async fn load() -> RundeckResult<RundeckConfig> {
    let fresh = read_file()?;
    let mut w = cache().write().await;
    w.cfg = fresh.clone();
    w.seen_mtime = config_path().as_ref().and_then(mtime_of);
    Ok(fresh)
}

pub async fn save(cfg: RundeckConfig) -> RundeckResult<()> {
    write_file(&cfg)?;
    let path = config_path();
    let mut w = cache().write().await;
    w.cfg = cfg;
    w.seen_mtime = path.as_ref().and_then(mtime_of);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_schemes_credentials_and_plaintext_policy() {
        assert!(validate_base_url("https://rundeck.example.com").is_ok());
        assert!(validate_base_url("http://localhost:4440").is_err());
        assert!(validate_base_url("http://rundeck.example.com").is_err());
        assert!(validate_base_url_with_policy("http://localhost:4440", true).is_ok());
        assert!(validate_base_url("ftp://rundeck.example.com").is_err());
        assert!(validate_base_url("https://user:pass@rundeck.example.com").is_err());
    }

    #[test]
    fn unquotes_every_form_the_cli_writes() {
        assert_eq!(unquote("'it'\\''s'"), "it's");
        assert_eq!(unquote("$'a\\nb'"), "a\nb");
        assert_eq!(unquote("\"plain\""), "plain");
        assert_eq!(unquote("''"), "");
        assert_eq!(unquote("'"), "'");
        assert_eq!(unquote("bare"), "bare");
        assert_eq!(unquote(&quote("tök'en")), "tök'en");
    }

    #[test]
    fn writes_atomically_and_keeps_lines_it_does_not_own() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rd-config");
        fs::write(
            &path,
            "# written by rnd\nRD_URL='https://old.example.com'\nRD_PASSWORD='cli-secret'\nRD_PROJECT=ops\nRD_TOKEN='old'\nRD_TOKEN='older'\n",
        )
        .unwrap();
        let cfg = RundeckConfig {
            url: "https://rundeck.example.com".into(),
            user: "alice".into(),
            token: "token".into(),
            token_id: "abc".into(),
            allow_insecure_private_http: false,
        };
        write_file_at(&path, &cfg).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            "# written by rnd\nRD_URL='https://rundeck.example.com'\nRD_PASSWORD='cli-secret'\nRD_PROJECT=ops\nRD_TOKEN='token'\nRD_USER='alice'\nRD_TOKEN_ID='abc'\nRD_ALLOW_INSECURE_PRIVATE_HTTP='0'\n"
        );
        let reread = parse(&text);
        assert_eq!(reread.token_id, "abc");
        assert_eq!(reread.url, "https://rundeck.example.com");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn a_cleared_config_writes_without_a_valid_url() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rd-config");
        let cfg = RundeckConfig {
            url: "not a url".into(),
            ..RundeckConfig::default()
        };
        write_file_at(&path, &cfg).unwrap();
        assert!(!fs::read_to_string(&path).unwrap().contains("RD_PASSWORD"));
    }
}
