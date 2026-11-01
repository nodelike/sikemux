// A database's address read as a draft of its profile, and a draft read back as a profile, or
// what it still needs.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::engines::mysql::MYSQL_PORT;
use crate::error::{DatabaseError, DatabaseResult};
use crate::profiles::{Draft, Server, Target, Tls, POSTGRES_PORT};

/// A connection before it is saved, in the fields its address gives.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDraft {
    /// None until it is first saved.
    pub id: Option<String>,
    pub name: String,
    pub engine: Engine,
    pub host: String,
    /// As typed; empty for the engine's usual port.
    pub port: String,
    pub database: String,
    pub user: String,
    pub tls: String,
    pub path: String,
    pub read_only: bool,
    /// Agents may run statements that change data; never while it is read only.
    pub agent_writes: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Postgres,
    Mysql,
    Sqlite,
}

/// A connection URL pasted into the form, and the form as it stood.
#[derive(Deserialize)]
pub struct UrlAsk {
    pub url: String,
    pub profile: ProfileDraft,
}

/// The form with what a connection URL says filled in, and the password it carried.
#[derive(Serialize)]
pub struct FromUrl {
    pub profile: ProfileDraft,
    pub password: Option<String>,
}

const TLS_MODES: [(Tls, &str, &str, &str); 4] = [
    (
        Tls::Prefer,
        "prefer",
        "Prefer",
        "Encrypt when the server offers it",
    ),
    (
        Tls::Require,
        "require",
        "Require",
        "Always encrypt, any certificate",
    ),
    (
        Tls::VerifyFull,
        "verify-full",
        "Verify",
        "Always encrypt, trusted certificate only",
    ),
    (Tls::Disable, "disable", "Off", "Never encrypt"),
];

/// A new PostgreSQL connection to every database on this host, as its usual user on its usual
/// port, encrypted when the server offers it.
pub fn blank() -> ProfileDraft {
    ProfileDraft {
        id: None,
        name: String::new(),
        engine: Engine::Postgres,
        host: "localhost".into(),
        port: POSTGRES_PORT.to_string(),
        database: String::new(),
        user: "postgres".into(),
        tls: "prefer".into(),
        path: String::new(),
        read_only: false,
        agent_writes: false,
    }
}

/// Fills the form from a URL such as `postgres://app:secret@db.internal:5432/shop?sslmode=require`,
/// `mysql://root@localhost/shop` or `sqlite:///Users/me/app.db`.
pub fn from_url(ask: UrlAsk) -> DatabaseResult<FromUrl> {
    let unreadable =
        || DatabaseError::BadArg("Not a postgres://, mysql:// or sqlite:// URL.".into());
    let url = ask.url.trim();
    let (scheme, rest) = url.split_once(':').ok_or_else(unreadable)?;
    let engine = match scheme.to_ascii_lowercase().as_str() {
        "postgres" | "postgresql" => Engine::Postgres,
        "mysql" | "mariadb" => Engine::Mysql,
        "sqlite" | "sqlite3" | "file" => Engine::Sqlite,
        _ => return Err(unreadable()),
    };
    let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
    let mut profile = ProfileDraft {
        engine,
        ..ask.profile
    };
    if engine == Engine::Sqlite {
        profile.path = decoded(rest.strip_prefix("//").unwrap_or(rest));
        if profile.name.trim().is_empty() {
            profile.name = Path::new(&profile.path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        return Ok(FromUrl {
            profile,
            password: None,
        });
    }
    let rest = rest.strip_prefix("//").ok_or_else(unreadable)?;
    let (authority, database) = rest.split_once('/').unwrap_or((rest, ""));
    let (login, address) = match authority.rsplit_once('@') {
        Some((login, address)) => (Some(login), address),
        None => (None, authority),
    };
    let (user, password) = match login.map(|login| login.split_once(':')) {
        Some(Some((user, password))) => (decoded(user), Some(decoded(password))),
        Some(None) => (decoded(login.unwrap_or_default()), None),
        None => (profile.user.clone(), None),
    };
    let (host, port) = match address.strip_prefix('[') {
        Some(bracketed) => {
            let (host, after) = bracketed.split_once(']').ok_or_else(unreadable)?;
            (host, after.strip_prefix(':').unwrap_or_default())
        }
        None => address.split_once(':').unwrap_or((address, "")),
    };
    if host.is_empty() {
        return Err(DatabaseError::BadArg("The URL names no host.".into()));
    }
    profile.host = decoded(host);
    profile.port = match (port, engine) {
        ("", Engine::Mysql) => MYSQL_PORT.to_string(),
        ("", _) => POSTGRES_PORT.to_string(),
        (port, _) => port.into(),
    };
    profile.user = user;
    profile.database = decoded(database.trim_end_matches('/'));
    for (key, value) in query.split('&').filter_map(|pair| pair.split_once('=')) {
        let key = key.to_ascii_lowercase().replace(['-', '_'], "");
        if let Some(tls) = (key == "sslmode" || key == "ssl")
            .then(|| tls_named(value))
            .flatten()
        {
            profile.tls = tls.into();
        }
    }
    if profile.name.trim().is_empty() {
        profile.name = match profile.database.is_empty() {
            true => profile.host.clone(),
            false => profile.database.clone(),
        };
    }
    Ok(FromUrl { profile, password })
}

/// The form's encryption for a URL's `sslmode`, as PostgreSQL and MySQL each name it.
fn tls_named(mode: &str) -> Option<&'static str> {
    match mode.to_ascii_lowercase().replace('_', "-").as_str() {
        "disable" | "disabled" | "false" | "0" => Some("disable"),
        "allow" | "prefer" | "preferred" => Some("prefer"),
        "require" | "required" | "true" | "1" => Some("require"),
        "verify-ca" | "verify-full" | "verify-identity" => Some("verify-full"),
        _ => None,
    }
}

/// `text` with its `%xx` escapes turned back into the characters they stand for.
fn decoded(text: &str) -> String {
    let mut out = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&byte, after)) = rest.split_first() {
        let escaped = after
            .get(..2)
            .filter(|_| byte == b'%')
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(escaped) => {
                out.push(escaped);
                rest = after.get(2..).unwrap_or_default();
            }
            None => {
                out.push(byte);
                rest = after;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The port as typed: empty is the engine's usual one; anything else must be 1 to 65535.
fn port(typed: &str) -> Result<Option<u16>, ()> {
    match typed.trim() {
        "" => Ok(None),
        digits => digits
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .map(Some)
            .ok_or(()),
    }
}

/// What still has to be filled in before the connection can be tried or saved.
fn missing(draft: &ProfileDraft) -> Option<&'static str> {
    if draft.name.trim().is_empty() {
        return Some("a name");
    }
    if draft.engine == Engine::Sqlite {
        return draft.path.trim().is_empty().then_some("the database file");
    }
    if draft.host.trim().is_empty() {
        return Some("a host");
    }
    if draft.user.trim().is_empty() {
        return Some("a user name");
    }
    port(&draft.port)
        .is_err()
        .then_some("a port from 1 to 65535")
}

/// The filled-in form as a profile to try or save, or what it still needs.
pub fn profile(draft: ProfileDraft) -> DatabaseResult<Draft> {
    if let Some(missing) = missing(&draft) {
        return Err(DatabaseError::BadArg(format!("Needs {missing}.")));
    }
    let server = || Server {
        host: draft.host.clone(),
        port: port(&draft.port).ok().flatten(),
        database: draft.database.clone(),
        user: draft.user.clone(),
        tls: TLS_MODES
            .iter()
            .find(|(_, value, ..)| *value == draft.tls)
            .map_or(Tls::Prefer, |(tls, ..)| *tls),
    };
    let target = match draft.engine {
        Engine::Postgres => Target::Postgres(server()),
        Engine::Mysql => Target::Mysql(server()),
        Engine::Sqlite => Target::Sqlite {
            path: draft.path.clone(),
        },
    };
    Ok(Draft {
        id: draft.id,
        name: draft.name,
        read_only: draft.read_only,
        agent_writes: draft.agent_writes && !draft.read_only,
        target,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shop() -> ProfileDraft {
        ProfileDraft {
            name: "Shop".into(),
            host: "db.internal".into(),
            port: String::new(),
            database: "shop".into(),
            user: "app".into(),
            ..blank()
        }
    }

    #[test]
    fn names_the_first_thing_still_missing() {
        let sqlite = ProfileDraft {
            name: "Local".into(),
            engine: Engine::Sqlite,
            ..blank()
        };
        assert_eq!(missing(&blank()), Some("a name"));
        assert_eq!(
            missing(&ProfileDraft {
                host: " ".into(),
                ..shop()
            }),
            Some("a host")
        );
        assert_eq!(
            missing(&ProfileDraft {
                user: String::new(),
                ..shop()
            }),
            Some("a user name")
        );
        assert_eq!(
            missing(&ProfileDraft {
                port: "70000".into(),
                ..shop()
            }),
            Some("a port from 1 to 65535")
        );
        assert_eq!(
            missing(&ProfileDraft {
                port: "abc".into(),
                ..shop()
            }),
            Some("a port from 1 to 65535")
        );
        assert_eq!(missing(&shop()), None);
        assert_eq!(missing(&sqlite), Some("the database file"));
        assert_eq!(
            missing(&ProfileDraft {
                path: "/tmp/a.db".into(),
                ..sqlite
            }),
            None
        );
        assert_eq!(
            missing(&ProfileDraft {
                engine: Engine::Mysql,
                user: String::new(),
                ..shop()
            }),
            Some("a user name")
        );
    }

    #[test]
    fn reads_an_empty_port_as_the_usual_one_and_never_lets_a_read_only_one_change_data() {
        let saved = profile(ProfileDraft {
            port: " 6543 ".into(),
            read_only: true,
            agent_writes: true,
            ..shop()
        })
        .unwrap();
        assert!(!saved.agent_writes);
        let Target::Postgres(server) = saved.target else {
            panic!("not PostgreSQL");
        };
        assert_eq!(server.port, Some(6543));
        let Target::Postgres(usual) = profile(shop()).unwrap().target else {
            panic!("not PostgreSQL");
        };
        assert_eq!(usual.port, None);
    }
}
