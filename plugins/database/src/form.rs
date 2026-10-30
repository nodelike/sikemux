// The connection form: what it starts from, the choices it offers, and a filled-in form read
// back as a profile, or what it still needs.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_plugin_database_views::{Engine, EngineChoice, ProfileDraft, ProfileForm, TlsChoice};

use crate::engines::mysql::MYSQL_PORT;
use crate::error::{DatabaseError, DatabaseResult};
use crate::profiles::{self, Draft, Profile, Server, Target, Tls, POSTGRES_PORT};

#[derive(Deserialize, Default)]
pub struct FormAsk {
    /// The saved connection to edit; none for a new one.
    #[serde(default)]
    pub id: Option<String>,
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

/// A filled-in form as Save and Test send it.
#[derive(Deserialize)]
pub struct Filled {
    pub profile: ProfileDraft,
    /// Left out to keep the saved password; empty to forget it.
    #[serde(default)]
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

pub fn form(data_dir: &Path, ask: FormAsk) -> DatabaseResult<ProfileForm> {
    let (draft, has_password) = match ask.id {
        Some(id) => {
            let saved = profiles::load(data_dir)?.get(&id)?.clone();
            (draft_of(&saved), saved.has_password)
        }
        None => (blank(), false),
    };
    Ok(ProfileForm {
        draft,
        has_password,
        engines: engines(),
        tls_modes: TLS_MODES
            .iter()
            .map(|(_, value, label, detail)| TlsChoice {
                value: (*value).into(),
                label: (*label).into(),
                detail: (*detail).into(),
            })
            .collect(),
    })
}

fn engines() -> Vec<EngineChoice> {
    let server = |engine, label: &str, port, user: &str, database: &str| EngineChoice {
        engine,
        label: label.into(),
        default_port: Some(port),
        name_hint: "Production".into(),
        user_hint: user.into(),
        database_hint: database.into(),
        path_hint: String::new(),
    };
    vec![
        server(
            Engine::Postgres,
            "PostgreSQL",
            POSTGRES_PORT,
            "postgres",
            "All databases",
        ),
        server(Engine::Mysql, "MySQL", MYSQL_PORT, "root", "All databases"),
        EngineChoice {
            engine: Engine::Sqlite,
            label: "SQLite".into(),
            default_port: None,
            name_hint: "Local app data".into(),
            user_hint: String::new(),
            database_hint: String::new(),
            path_hint: "~/data/app.db".into(),
        },
    ]
}

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

fn draft_of(profile: &Profile) -> ProfileDraft {
    let mut draft = ProfileDraft {
        id: Some(profile.id.clone()),
        name: profile.name.clone(),
        read_only: profile.read_only,
        agent_writes: profile.agent_writes,
        ..blank()
    };
    match &profile.target {
        Target::Postgres(server) | Target::Mysql(server) => {
            draft.engine = match profile.target {
                Target::Mysql(_) => Engine::Mysql,
                _ => Engine::Postgres,
            };
            draft.host = server.host.clone();
            draft.port = server.port.map(|port| port.to_string()).unwrap_or_default();
            draft.database = server.database.clone();
            draft.user = server.user.clone();
            draft.tls = TLS_MODES
                .iter()
                .find(|(tls, ..)| *tls == server.tls)
                .map_or("prefer", |(_, value, ..)| value)
                .into();
        }
        Target::Sqlite { path } => {
            draft.engine = Engine::Sqlite;
            draft.path = path.clone();
        }
    }
    draft
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

    #[test]
    fn edits_a_saved_connection_as_it_was_saved() {
        let saved = Profile {
            id: "p1".into(),
            name: "Shop".into(),
            read_only: false,
            agent_writes: true,
            has_password: true,
            target: Target::Mysql(Server {
                host: "db.internal".into(),
                port: Some(6543),
                database: "shop".into(),
                user: "app".into(),
                tls: Tls::VerifyFull,
            }),
        };
        let draft = draft_of(&saved);
        assert_eq!(draft.engine, Engine::Mysql);
        assert_eq!(draft.port, "6543");
        assert_eq!(draft.tls, "verify-full");
        assert_eq!(profile(draft).unwrap().target, saved.target);
    }
}
