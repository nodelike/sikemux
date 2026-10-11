// The databases saved here. Passwords live in the Keychain; the file beside
// them only says where each database is and how to reach it.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::error::{DatabaseError, DatabaseResult};

#[cfg(not(test))]
const PASSWORD_SERVICE: &str = "sikemux-database-password";
/// Tests keep to an entry of their own, so they never replace or delete a real password.
#[cfg(test)]
const PASSWORD_SERVICE: &str = "sikemux-database-password-test";

pub const POSTGRES_PORT: u16 = 5432;

#[derive(Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum Tls {
    Disable,
    /// Encrypt when the server offers it, without checking its certificate.
    #[default]
    Prefer,
    /// Always encrypt, without checking the certificate.
    Require,
    /// Always encrypt, and check the certificate against the system's trusted roots.
    VerifyFull,
}

/// A database server reached over the network.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    pub host: String,
    /// Empty for the engine's usual port.
    #[serde(default)]
    pub port: Option<u16>,
    pub database: String,
    pub user: String,
    #[serde(default)]
    pub tls: Tls,
}

impl Server {
    /// `user@host:port/database`, as the person would recognise it.
    pub fn address(&self, default_port: u16) -> String {
        format!(
            "{}@{}:{}/{}",
            self.user,
            self.host,
            self.port.unwrap_or(default_port),
            self.database
        )
    }

    fn check(&self) -> DatabaseResult<()> {
        let missing = |what: &str| Err(DatabaseError::BadArg(format!("a {what} is needed")));
        if self.host.trim().is_empty() {
            return missing("host");
        }
        if self.user.trim().is_empty() {
            return missing("user name");
        }
        Ok(())
    }

    fn same_sign_in(&self, other: &Self) -> bool {
        self.host == other.host && self.port == other.port && self.user == other.user
    }

    fn trimmed(self) -> Self {
        Self {
            host: self.host.trim().to_string(),
            database: self.database.trim().to_string(),
            user: self.user.trim().to_string(),
            ..self
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(tag = "engine", rename_all = "lowercase")]
pub enum Target {
    Postgres(Server),
    Mysql(Server),
    Sqlite { path: String },
}

impl Target {
    fn check(&self) -> DatabaseResult<()> {
        match self {
            Self::Postgres(server) | Self::Mysql(server) => server.check(),
            Self::Sqlite { path } if path.trim().is_empty() => {
                Err(DatabaseError::BadArg("a database file is needed".into()))
            }
            Self::Sqlite { .. } => Ok(()),
        }
    }

    /// Whether a password saved for one would sign in to the other: the same engine, host, port and user.
    fn same_sign_in(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Postgres(saved), Self::Postgres(typed))
            | (Self::Mysql(saved), Self::Mysql(typed)) => saved.same_sign_in(typed),
            _ => false,
        }
    }

    fn trimmed(self) -> Self {
        match self {
            Self::Postgres(server) => Self::Postgres(server.trimmed()),
            Self::Mysql(server) => Self::Mysql(server.trimmed()),
            Self::Sqlite { path } => Self::Sqlite {
                path: path.trim().to_string(),
            },
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// Opens every connection read-only, a guard against accidental changes. A statement can switch it off
    /// for the person's own session; agents get a connection where it cannot be switched off.
    #[serde(default)]
    pub read_only: bool,
    /// Lets agents run statements that change data. Without it an agent's connection is read-only.
    #[serde(default)]
    pub agent_writes: bool,
    #[serde(default)]
    pub has_password: bool,
    #[serde(flatten)]
    pub target: Target,
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq, Debug)]
pub struct Profiles {
    #[serde(default)]
    pub profiles: Vec<Profile>,
}

impl Profiles {
    pub fn get(&self, id: &str) -> DatabaseResult<&Profile> {
        self.profiles
            .iter()
            .find(|profile| profile.id == id)
            .ok_or_else(|| DatabaseError::NotFound(format!("no saved database with id {id}")))
    }

    /// The saved database named by id, or failing that by name in any case, as an agent would name it.
    pub fn find(&self, wanted: &str) -> DatabaseResult<&Profile> {
        let wanted = wanted.trim();
        self.profiles
            .iter()
            .find(|profile| profile.id == wanted)
            .or_else(|| {
                self.profiles
                    .iter()
                    .find(|profile| profile.name.eq_ignore_ascii_case(wanted))
            })
            .ok_or_else(|| {
                let names: Vec<&str> = self
                    .profiles
                    .iter()
                    .map(|profile| profile.name.as_str())
                    .collect();
                DatabaseError::NotFound(format!(
                    "no saved database named {wanted}; the saved ones are: {}",
                    if names.is_empty() {
                        "none".to_string()
                    } else {
                        names.join(", ")
                    }
                ))
            })
    }

    fn upsert(&mut self, profile: Profile) {
        match self.profiles.iter_mut().find(|kept| kept.id == profile.id) {
            Some(kept) => *kept = profile,
            None => self.profiles.push(profile),
        }
    }

    fn remove(&mut self, id: &str) -> Option<Profile> {
        let index = self.profiles.iter().position(|profile| profile.id == id)?;
        Some(self.profiles.remove(index))
    }
}

/// A profile as the form sends it: no id yet when it is new.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub agent_writes: bool,
    #[serde(flatten)]
    pub target: Target,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SaveRequest {
    pub profile: Draft,
    /// Left out to keep the saved password; empty to forget it.
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct IdRequest {
    pub id: String,
}

fn profiles_path(data_dir: &Path) -> PathBuf {
    data_dir.join("profiles.json")
}

/// Saves and removals each read the file, change it and write it back, so they take turns.
static WRITING: Mutex<()> = Mutex::new(());

/// The saved databases; none when nothing was saved yet. A file that cannot be read is an error, so a save
/// never writes over connections it could not read.
pub fn load(data_dir: &Path) -> DatabaseResult<Profiles> {
    let path = profiles_path(data_dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Profiles::default())
        }
        Err(error) => return Err(error.into()),
    };
    serde_json::from_slice(&bytes).map_err(|error| {
        DatabaseError::Storage(format!("{} could not be read: {error}", path.display()))
    })
}

fn store(data_dir: &Path, profiles: &Profiles) -> DatabaseResult<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = profiles_path(data_dir);
    let staged = path.with_extension("json.tmp");
    std::fs::write(&staged, serde_json::to_vec_pretty(profiles)?)?;
    std::fs::rename(&staged, &path)?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum PasswordChange<'a> {
    Keep,
    Forget,
    Set(&'a str),
}

/// A password of only spaces counts as none, but one with spaces around it is kept as typed.
fn password_change(password: Option<&str>) -> PasswordChange<'_> {
    match password {
        None => PasswordChange::Keep,
        Some(password) if password.trim().is_empty() => PasswordChange::Forget,
        Some(password) => PasswordChange::Set(password),
    }
}

/// Saves the profile and its password, and returns it as saved.
pub fn save(data_dir: &Path, request: SaveRequest) -> DatabaseResult<Profile> {
    let SaveRequest { profile, password } = request;
    let name = profile.name.trim().to_string();
    if name.is_empty() {
        return Err(DatabaseError::BadArg("a name is needed".into()));
    }
    let target = profile.target.trimmed();
    target.check()?;
    let _turn = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    let mut profiles = load(data_dir)?;
    let existing = profile
        .id
        .as_deref()
        .map(|id| profiles.get(id).cloned())
        .transpose()?;
    if profiles
        .profiles
        .iter()
        .any(|kept| kept.name.eq_ignore_ascii_case(&name) && Some(&kept.id) != profile.id.as_ref())
    {
        return Err(DatabaseError::BadArg(format!(
            "a database named {name} is already saved"
        )));
    }
    let id = existing
        .as_ref()
        .map_or_else(|| uuid::Uuid::new_v4().to_string(), |kept| kept.id.clone());
    let has_password = match password_change(password.as_deref()) {
        PasswordChange::Forget => {
            password_delete(&id)?;
            false
        }
        PasswordChange::Set(password) => {
            password_write(&id, password)?;
            true
        }
        PasswordChange::Keep => existing.as_ref().is_some_and(|kept| kept.has_password),
    };
    let saved = Profile {
        id,
        name,
        read_only: profile.read_only,
        agent_writes: profile.agent_writes && !profile.read_only,
        has_password,
        target,
    };
    profiles.upsert(saved.clone());
    store(data_dir, &profiles)?;
    Ok(saved)
}

pub fn remove(data_dir: &Path, id: &str) -> DatabaseResult<()> {
    let _turn = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    let mut profiles = load(data_dir)?;
    if let Some(removed) = profiles.remove(id) {
        store(data_dir, &profiles)?;
        if removed.has_password {
            password_delete(&removed.id)?;
        }
    }
    Ok(())
}

fn keychain_error(error: sikemux_keychain::KeychainError) -> DatabaseError {
    match error {
        sikemux_keychain::KeychainError::Invalid(message) => DatabaseError::BadArg(message),
        sikemux_keychain::KeychainError::Failed(message) => DatabaseError::Keychain(message),
    }
}

/// The Keychain helper takes only token-like text, and passwords can hold any character.
fn to_hex(text: &str) -> String {
    text.bytes().map(|byte| format!("{byte:02x}")).collect()
}

fn from_hex(hex: &str) -> Option<String> {
    let digits: Vec<u8> = hex
        .chars()
        .map(|c| c.to_digit(16).and_then(|digit| u8::try_from(digit).ok()))
        .collect::<Option<_>>()?;
    let bytes = digits
        .chunks(2)
        .map(|pair| match pair {
            [high, low] => Some(high * 16 + low),
            _ => None,
        })
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes).ok()
}

pub fn password_read(id: &str) -> DatabaseResult<Option<String>> {
    let stored = sikemux_keychain::read(PASSWORD_SERVICE, id).map_err(keychain_error)?;
    Ok(stored.as_deref().and_then(from_hex))
}

pub fn saved_password(profile: &Profile) -> DatabaseResult<Option<String>> {
    if profile.has_password {
        password_read(&profile.id)
    } else {
        Ok(None)
    }
}

/// The password to try a draft with: the one just typed, or else the saved one while the draft still signs in
/// to the same server as the same user.
pub fn password_for(
    data_dir: &Path,
    draft: &Draft,
    typed: Option<String>,
) -> DatabaseResult<Option<String>> {
    if typed.is_some() {
        return Ok(typed);
    }
    let Some(id) = draft.id.as_deref() else {
        return Ok(None);
    };
    let profiles = load(data_dir)?;
    match profiles.get(id) {
        Ok(saved) if saved.target.same_sign_in(&draft.target.clone().trimmed()) => {
            saved_password(saved)
        }
        _ => Ok(None),
    }
}

fn password_write(id: &str, password: &str) -> DatabaseResult<()> {
    sikemux_keychain::write(PASSWORD_SERVICE, id, &to_hex(password)).map_err(keychain_error)
}

fn password_delete(id: &str) -> DatabaseResult<()> {
    sikemux_keychain::delete(PASSWORD_SERVICE, id).map_err(keychain_error)
}

/// Runs Keychain and disk work on a thread meant for blocking, away from the few threads every plugin shares.
pub async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> DatabaseResult<T> + Send + 'static,
) -> DatabaseResult<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| DatabaseError::Storage(error.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sikemux-database-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn sqlite(name: &str, path: &str) -> SaveRequest {
        SaveRequest {
            profile: Draft {
                id: None,
                name: name.into(),
                read_only: false,
                agent_writes: false,
                target: Target::Sqlite { path: path.into() },
            },
            password: None,
        }
    }

    #[test]
    fn a_password_survives_the_round_trip_through_hex() {
        for password in ["", "plain", "p@ss w0rd!#$%^&*()\"'", "pässwörd 🔑"] {
            assert_eq!(from_hex(&to_hex(password)).as_deref(), Some(password));
        }
        assert_eq!(from_hex("abc"), None);
        assert_eq!(from_hex("zz"), None);
    }

    #[test]
    fn a_typed_password_wins_and_no_saved_one_means_none() {
        let dir = scratch("password-for");
        let saved = save(&dir, sqlite("Local", "/tmp/a.db")).unwrap();
        let mut draft = sqlite("Local", "/tmp/a.db").profile;
        draft.id = Some(saved.id);
        assert_eq!(
            password_for(&dir, &draft, Some("typed".into())).unwrap(),
            Some("typed".into())
        );
        assert_eq!(password_for(&dir, &draft, None).unwrap(), None);
        draft.id = None;
        assert_eq!(password_for(&dir, &draft, None).unwrap(), None);
    }

    #[test]
    fn a_password_is_saved_as_typed_and_only_blank_forgets_it() {
        assert_eq!(password_change(None), PasswordChange::Keep);
        assert_eq!(password_change(Some("   ")), PasswordChange::Forget);
        assert_eq!(password_change(Some("")), PasswordChange::Forget);
        assert_eq!(
            password_change(Some(" secret ")),
            PasswordChange::Set(" secret ")
        );
    }

    #[test]
    fn a_saved_password_is_reused_only_for_the_same_server_and_user() {
        let server = |host: &str, port: Option<u16>, user: &str| Server {
            host: host.into(),
            port,
            database: "app".into(),
            user: user.into(),
            tls: Tls::Prefer,
        };
        let saved = Target::Postgres(server("db", None, "app"));
        let mut renamed = server("db", None, "app");
        renamed.database = "other".into();
        assert!(saved.same_sign_in(&Target::Postgres(renamed)));
        assert!(!saved.same_sign_in(&Target::Postgres(server("other", None, "app"))));
        assert!(!saved.same_sign_in(&Target::Postgres(server("db", Some(6543), "app"))));
        assert!(!saved.same_sign_in(&Target::Postgres(server("db", None, "admin"))));
        assert!(!saved.same_sign_in(&Target::Mysql(server("db", None, "app"))));
    }

    #[test]
    fn a_profiles_file_that_cannot_be_read_is_an_error_and_never_overwritten() {
        let dir = scratch("unreadable");
        assert!(load(&dir).unwrap().profiles.is_empty());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(profiles_path(&dir), "{ not json").unwrap();
        assert!(matches!(load(&dir), Err(DatabaseError::Storage(_))));
        assert!(save(&dir, sqlite("Local", "/tmp/a.db")).is_err());
        assert_eq!(
            std::fs::read_to_string(profiles_path(&dir)).unwrap(),
            "{ not json"
        );
    }

    #[test]
    fn profiles_read_and_write_with_the_engine_beside_the_fields() {
        let json = r#"{"id":"1","name":"Shop","engine":"postgres","host":"localhost","database":"shop","user":"app","tls":"verify-full"}"#;
        let profile: Profile = serde_json::from_str(json).unwrap();
        assert_eq!(
            profile.target,
            Target::Postgres(Server {
                host: "localhost".into(),
                port: None,
                database: "shop".into(),
                user: "app".into(),
                tls: Tls::VerifyFull,
            })
        );
        let written = serde_json::to_value(&profile).unwrap();
        assert_eq!(written["engine"], "postgres");
        assert_eq!(written["host"], "localhost");
        assert_eq!(written["readOnly"], false);
    }

    #[test]
    fn a_mysql_profile_reads_with_its_engine_name() {
        let json = r#"{"id":"2","name":"Legacy","engine":"mysql","host":"db","port":3307,"database":"app","user":"root"}"#;
        let profile: Profile = serde_json::from_str(json).unwrap();
        let Target::Mysql(server) = &profile.target else {
            panic!("expected a MySQL target")
        };
        assert_eq!(server.address(3306), "root@db:3307/app");
        assert_eq!(server.tls, Tls::Prefer);
    }

    #[test]
    fn saving_gives_a_new_profile_an_id_and_keeps_it_on_edit() {
        let dir = scratch("save");
        let saved = save(&dir, sqlite(" Local ", " /tmp/a.db ")).unwrap();
        assert_eq!(saved.name, "Local");
        assert_eq!(
            saved.target,
            Target::Sqlite {
                path: "/tmp/a.db".into()
            }
        );
        let mut edit = sqlite("Local copy", "/tmp/b.db");
        edit.profile.id = Some(saved.id.clone());
        let edited = save(&dir, edit).unwrap();
        assert_eq!(edited.id, saved.id);
        assert_eq!(load(&dir).unwrap().profiles, vec![edited]);
        remove(&dir, &saved.id).unwrap();
        assert!(load(&dir).unwrap().profiles.is_empty());
    }

    #[test]
    fn agents_may_change_data_only_where_people_may_too() {
        let dir = scratch("agent-writes");
        let mut allowed = sqlite("Writable", "/tmp/a.db");
        allowed.profile.agent_writes = true;
        assert!(save(&dir, allowed).unwrap().agent_writes);
        let mut locked = sqlite("Locked", "/tmp/b.db");
        locked.profile.agent_writes = true;
        locked.profile.read_only = true;
        assert!(!save(&dir, locked).unwrap().agent_writes);
        let written = serde_json::to_value(&load(&dir).unwrap().profiles[0]).unwrap();
        assert_eq!(written["agentWrites"], true);
    }

    #[test]
    fn a_profile_is_found_by_id_or_by_name_in_any_case() {
        let dir = scratch("find");
        let saved = save(&dir, sqlite("Analytics", "/tmp/a.db")).unwrap();
        let profiles = load(&dir).unwrap();
        assert_eq!(profiles.find(&saved.id).unwrap().name, "Analytics");
        assert_eq!(profiles.find(" analytics ").unwrap().id, saved.id);
        let Err(DatabaseError::NotFound(message)) = profiles.find("other") else {
            panic!("expected not found")
        };
        assert!(message.ends_with("Analytics"), "{message}");
    }

    #[test]
    fn names_are_unique_and_required_fields_are_checked() {
        let dir = scratch("checks");
        save(&dir, sqlite("Local", "/tmp/a.db")).unwrap();
        assert!(matches!(
            save(&dir, sqlite("local", "/tmp/b.db")),
            Err(DatabaseError::BadArg(_))
        ));
        assert!(matches!(
            save(&dir, sqlite("  ", "/tmp/b.db")),
            Err(DatabaseError::BadArg(_))
        ));
        assert!(matches!(
            save(&dir, sqlite("Other", " ")),
            Err(DatabaseError::BadArg(_))
        ));
        let mut unknown = sqlite("Ghost", "/tmp/c.db");
        unknown.profile.id = Some("nope".into());
        assert!(matches!(
            save(&dir, unknown),
            Err(DatabaseError::NotFound(_))
        ));
    }
}
