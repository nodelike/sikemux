//! What a plugin keeps between calls: its settings file, the accounts listed
//! in it, its secrets in the Keychain, and secrets read from there lately.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::Serialize;
use sikemux_process::sync::Locked;

use crate::service::ServiceError;

/// The Keychain service a plugin's secrets live under. A development build
/// keeps entries of its own, so it never spends or replaces the installed
/// app's sign-in, and tests keep to theirs, so they never touch a real one.
#[macro_export]
macro_rules! keychain_service {
    ($name:literal) => {
        if cfg!(test) {
            concat!($name, "-test")
        } else if cfg!(debug_assertions) {
            concat!($name, "-dev")
        } else {
            $name
        }
    };
}

/// Runs work that starts a process or touches the Keychain or the disk on a
/// thread meant for blocking, away from the few threads every plugin shares.
pub async fn blocking<T, S>(
    work: impl FnOnce() -> Result<T, ServiceError<S>> + Send + 'static,
) -> Result<T, ServiceError<S>>
where
    T: Send + 'static,
    S: 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| ServiceError::Transport(error.to_string()))?
}

#[derive(Debug)]
pub struct SettingsError(String);

impl fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<S> From<SettingsError> for ServiceError<S> {
    fn from(error: SettingsError) -> Self {
        Self::Transport(error.0)
    }
}

fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("config.json")
}

/// What a plugin saved in `path`, or the empty value before anything was. A
/// file that cannot be read is an error rather than empty, so the next save
/// never writes over what it held.
pub fn read_file<T: DeserializeOwned + Default>(path: &Path) -> Result<T, SettingsError> {
    let unreadable = |error: &dyn fmt::Display| {
        SettingsError(format!("{} cannot be read: {error}", path.display()))
    };
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| unreadable(&error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(unreadable(&error)),
    }
}

/// Writes beside the file and moves it into place, so a crash never leaves half of it.
pub fn write_file<T: Serialize>(path: &Path, value: &T) -> Result<(), SettingsError> {
    let failed =
        |error: &dyn fmt::Display| SettingsError(format!("saving {}: {error}", path.display()));
    let staged = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| failed(&error))?;
    path.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&staged, bytes))
        .and_then(|()| std::fs::rename(&staged, path))
        .map_err(|error| failed(&error))
}

/// The plugin's settings file.
pub fn load<T: DeserializeOwned + Default>(data_dir: &Path) -> Result<T, SettingsError> {
    read_file(&settings_path(data_dir))
}

pub fn save<T: Serialize>(data_dir: &Path, settings: &T) -> Result<(), SettingsError> {
    write_file(&settings_path(data_dir), settings)
}

/// Reads the settings, changes them and saves them, all on a blocking thread,
/// so a change made meanwhile by another call is never written over.
pub async fn update<C, T, S>(
    data_dir: &Path,
    change: impl FnOnce(&mut C) -> Result<T, ServiceError<S>> + Send + 'static,
) -> Result<T, ServiceError<S>>
where
    C: Serialize + DeserializeOwned + Default,
    T: Send + 'static,
    S: 'static,
{
    let data_dir = data_dir.to_path_buf();
    blocking(move || {
        let mut settings: C = load(&data_dir)?;
        let changed = change(&mut settings)?;
        save(&data_dir, &settings)?;
        Ok(changed)
    })
    .await
}

/// A secret read from the Keychain is held this long, since starting
/// `security` for every request costs more than the request.
pub const SECRET_LIFE: Duration = Duration::from_secs(60);

/// Values read lately, each kept for a while by its key.
pub struct Held<V> {
    values: Mutex<BTreeMap<String, (V, Instant)>>,
    life: Duration,
}

impl<V: Clone> Held<V> {
    pub const fn new(life: Duration) -> Self {
        Self {
            values: Mutex::new(BTreeMap::new()),
            life,
        }
    }

    pub const fn secrets() -> Self {
        Self::new(SECRET_LIFE)
    }

    pub fn get(&self, key: &str) -> Option<V> {
        let values = self.values.locked();
        let (value, at) = values.get(key)?;
        (at.elapsed() < self.life).then(|| value.clone())
    }

    pub fn put(&self, key: &str, value: V) {
        self.values
            .locked()
            .insert(key.to_owned(), (value, Instant::now()));
    }

    /// Drops one key, or every one, so the next read goes back to the source.
    pub fn forget(&self, key: Option<&str>) {
        let mut values = self.values.locked();
        match key {
            Some(key) => {
                values.remove(key);
            }
            None => values.clear(),
        }
    }
}

pub trait Account: Clone {
    fn id(&self) -> &str;

    /// Whether a call naming `name` means this account.
    fn is_named(&self, name: &str) -> bool {
        self.id() == name
    }
}

/// Settings that list the accounts signed in and which one a call uses when
/// it names none.
pub trait Accounts {
    type Account: Account;

    fn accounts(&self) -> &[Self::Account];
    fn accounts_mut(&mut self) -> &mut Vec<Self::Account>;
    fn default_id(&self) -> Option<&str>;
    fn default_id_mut(&mut self) -> &mut Option<String>;

    /// The account named, or with none named, the default one.
    fn account(&self, name: Option<&str>) -> Option<&Self::Account> {
        let wanted = name.filter(|name| !name.is_empty()).or(self.default_id());
        match wanted {
            Some(wanted) => self
                .accounts()
                .iter()
                .find(|account| account.is_named(wanted)),
            None => self.accounts().first(),
        }
    }

    /// Adds the account, or replaces the one with the same id. The first account becomes the default.
    fn upsert(&mut self, account: Self::Account) {
        let accounts = self.accounts_mut();
        match accounts.iter_mut().find(|kept| kept.id() == account.id()) {
            Some(kept) => *kept = account,
            None => accounts.push(account),
        }
        let default_is_signed_in = self
            .default_id()
            .is_some_and(|id| self.accounts().iter().any(|account| account.id() == id));
        if !default_is_signed_in {
            *self.default_id_mut() = self
                .accounts()
                .first()
                .map(|account| account.id().to_owned());
        }
    }

    /// Takes the account out; a removed default passes to the first account left.
    fn remove(&mut self, id: &str) -> Option<Self::Account> {
        let index = self
            .accounts()
            .iter()
            .position(|account| account.id() == id)?;
        let removed = self.accounts_mut().remove(index);
        if self.default_id() == Some(id) {
            *self.default_id_mut() = self
                .accounts()
                .first()
                .map(|account| account.id().to_owned());
        }
        Some(removed)
    }

    /// Makes a signed-in account the one used when a call names none.
    fn set_default<S>(&mut self, id: &str) -> Result<(), ServiceError<S>> {
        if !self.accounts().iter().any(|account| account.id() == id) {
            return Err(ServiceError::NotFound(format!("{id} is not signed in")));
        }
        *self.default_id_mut() = Some(id.to_owned());
        Ok(())
    }

    /// Every account, the default first, which is the order to try them in.
    fn in_order(&self) -> Vec<Self::Account> {
        let mut ordered = self.accounts().to_vec();
        ordered.sort_by_key(|account| Some(account.id()) != self.default_id());
        ordered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
    struct Book {
        accounts: Vec<String>,
        default: Option<String>,
    }

    impl Account for String {
        fn id(&self) -> &str {
            self
        }
    }

    impl Accounts for Book {
        type Account = String;

        fn accounts(&self) -> &[String] {
            &self.accounts
        }
        fn accounts_mut(&mut self) -> &mut Vec<String> {
            &mut self.accounts
        }
        fn default_id(&self) -> Option<&str> {
            self.default.as_deref()
        }
        fn default_id_mut(&mut self) -> &mut Option<String> {
            &mut self.default
        }
    }

    #[test]
    fn the_first_account_is_the_default_until_it_leaves() {
        let mut book = Book::default();
        assert!(book.account(None).is_none());
        book.upsert("one".into());
        book.upsert("two".into());
        assert_eq!(book.account(None).map(String::as_str), Some("one"));
        assert!(book.set_default::<()>("two").is_ok());
        assert!(book.set_default::<()>("three").is_err());
        assert_eq!(book.in_order(), ["two", "one"]);
        book.remove("two");
        assert_eq!(book.default.as_deref(), Some("one"));
    }

    #[test]
    fn a_settings_file_that_cannot_be_read_is_not_taken_for_empty_settings() -> std::io::Result<()>
    {
        let dir = std::env::temp_dir().join(format!("sikemux-store-{}", std::process::id()));
        assert!(load::<Book>(&dir).is_ok_and(|book| book.accounts.is_empty()));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("config.json"), b"{ not json")?;
        assert!(load::<Book>(&dir).is_err());
        std::fs::remove_dir_all(dir)
    }
}
