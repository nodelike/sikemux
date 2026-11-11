//! How a plugin's sign-ins reach the account vault. A plugin describes where it keeps each
//! sign-in with a [`Book`]; the core, when the host is on an account, sets a [`Keeper`] that
//! decides who may use a secret, logs each use, and hears when one changes. With no keeper,
//! as on a host with no account, secrets stay in this host's secret store alone.

use std::cell::RefCell;
use std::future::Future;
use std::marker::PhantomData;
use std::path::Path;
use std::sync::OnceLock;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sikemux_process::sync::Locked;

use crate::store::{self, Secrets};
use crate::PluginFuture;

/// Who a plugin call is made for: an agent in a project, or the host itself when neither is set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Caller {
    pub agent: Option<String>,
    pub project: Option<String>,
}

tokio::task_local! {
    static CALLER: Caller;
}

thread_local! {
    static ON_THREAD: RefCell<Option<Caller>> = const { RefCell::new(None) };
}

/// Runs `work` on behalf of `caller`.
pub fn as_caller<F: Future>(caller: Caller, work: F) -> impl Future<Output = F::Output> {
    CALLER.scope(caller, work)
}

pub fn caller() -> Option<Caller> {
    CALLER
        .try_with(Clone::clone)
        .ok()
        .or_else(|| ON_THREAD.with(|on| on.borrow().clone()))
}

/// Wraps work bound for another thread so it keeps the caller of the task that made it.
pub fn carry<T>(work: impl FnOnce() -> T + Send) -> impl FnOnce() -> T + Send {
    let caller = caller();
    move || {
        let before = ON_THREAD.with(|on| on.replace(caller));
        let done = work();
        ON_THREAD.with(|on| *on.borrow_mut() = before);
        done
    }
}

/// An access token the host that refreshes a sign-in handed on, for the hosts that only read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedAccess {
    pub token: String,
    #[serde(default)]
    pub scopes: String,
    /// Milliseconds since the Unix epoch.
    pub expires_at: u64,
}

pub enum Refresh {
    /// This host refreshes the sign-in itself.
    Here,
    /// Another host refreshes it; this is the token it handed on.
    Use(SharedAccess),
    /// Another host refreshes it and has not handed on a token that still works.
    Wait(String),
}

pub trait Keeper: Send + Sync {
    /// Whether `caller` may use the secret kept under `service` and `account`. A use that is
    /// allowed is logged.
    fn using(&self, service: &str, account: &str, caller: Option<&Caller>) -> Result<(), String>;
    /// A plugin wrote or deleted the secret.
    fn changed(&self, service: &str, account: &str);
    fn refresh(&self, service: &str, account: &str) -> Refresh;
    /// This host refreshed the sign-in and has a new access token to hand on.
    fn refreshed(&self, service: &str, account: &str, access: SharedAccess);
}

static KEEPER: OnceLock<&'static dyn Keeper> = OnceLock::new();

pub fn keep(keeper: &'static dyn Keeper) {
    let _ = KEEPER.set(keeper);
}

pub(crate) fn keeper() -> Option<&'static dyn Keeper> {
    KEEPER.get().copied()
}

pub(crate) fn using(service: &str, account: &str) -> Result<(), String> {
    match keeper() {
        Some(keeper) => keeper.using(service, account, caller().as_ref()),
        None => Ok(()),
    }
}

pub(crate) fn changed(service: &str, account: &str) {
    if let Some(keeper) = keeper() {
        keeper.changed(service, account);
    }
}

pub(crate) fn refresh(service: &str, account: &str) -> Refresh {
    keeper().map_or(Refresh::Here, |keeper| keeper.refresh(service, account))
}

pub(crate) fn refreshed(service: &str, account: &str, access: SharedAccess) {
    if let Some(keeper) = keeper() {
        keeper.refreshed(service, account, access);
    }
}

/// One secret in the host's secret store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    pub service: String,
    pub account: String,
    pub secret: String,
}

/// One sign-in as the vault holds it: the secrets it needs and what the plugin's settings keep
/// of it, so another host can put both back as they were.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignIn {
    /// The same on every host signed in as the same identity on the same service.
    pub identity: String,
    pub label: String,
    pub secrets: Vec<Kept>,
    pub records: Value,
}

/// Renews a sign-in's access token now, for the host that refreshes it.
pub trait Renew: Send + Sync {
    fn renew<'a>(&'a self, identity: &'a str) -> PluginFuture<'a, ()>;
}

/// Where a plugin keeps its sign-ins. Each method reads or writes this host's store directly,
/// never through the keeper.
pub trait Book: Send + Sync {
    fn sign_ins(&self, data_dir: &Path) -> Result<Vec<SignIn>, String>;
    /// Puts a sign-in from the vault here, in place of any with the same identity.
    fn take_in(&self, data_dir: &Path, sign_in: &SignIn) -> Result<(), String>;
    fn let_go(&self, data_dir: &Path, identity: &str) -> Result<(), String>;
    fn renewer(&self) -> Option<&'static dyn Renew> {
        None
    }
}

fn read_raw(secrets: Secrets, account: &str) -> Result<Option<String>, String> {
    sikemux_keychain::read(secrets.service(), account).map_err(|error| error.to_string())
}

fn write_kept(kept: &Kept) -> Result<(), String> {
    sikemux_keychain::write(&kept.service, &kept.account, &kept.secret)
        .map_err(|error| error.to_string())
}

fn delete_raw(service: &str, account: &str) -> Result<(), String> {
    sikemux_keychain::delete(service, account).map_err(|error| error.to_string())
}

fn read_settings(path: &Path) -> Result<Map<String, Value>, String> {
    store::read_file::<Map<String, Value>>(path).map_err(|error| error.to_string())
}

fn write_settings(path: &Path, settings: &Map<String, Value>) -> Result<(), String> {
    store::write_file(path, settings).map_err(|error| error.to_string())
}

/// A record in a plugin's list of accounts.
pub trait Listing: DeserializeOwned {
    /// The secret store account its secret is under; none when it keeps none there.
    fn secret_account(&self) -> Option<String>;
    fn label(&self) -> String;
    /// What it signs in as, the same on every host that signs in the same way. Records with
    /// the same identity make one sign-in.
    fn identity(&self) -> Option<String> {
        self.secret_account()
    }
}

/// A plugin whose settings file lists its accounts, each with its secret under one service.
/// Records that share a secret make one sign-in.
pub struct Listed<A> {
    file: &'static str,
    list: &'static str,
    default: Option<&'static str>,
    id: &'static str,
    secrets: Secrets,
    renewer: Option<&'static dyn Renew>,
    record: PhantomData<fn() -> A>,
}

impl<A: Listing> Listed<A> {
    /// `list` names the settings field holding the records, `id` each record's id field, and
    /// `default` the field holding the default record's id.
    pub const fn new(
        file: &'static str,
        list: &'static str,
        id: &'static str,
        default: Option<&'static str>,
        secrets: Secrets,
    ) -> Self {
        Self {
            file,
            list,
            default,
            id,
            secrets,
            renewer: None,
            record: PhantomData,
        }
    }

    pub const fn renewed_by(mut self, renewer: &'static dyn Renew) -> Self {
        self.renewer = Some(renewer);
        self
    }

    fn records(settings: &Map<String, Value>, list: &str) -> Vec<Value> {
        settings
            .get(list)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    fn read(record: &Value) -> Option<A> {
        serde_json::from_value::<A>(record.clone()).ok()
    }

    fn identity_of(record: &Value) -> Option<String> {
        Self::read(record)?.identity()
    }

    /// Where the secrets of the records with `identity` are kept here.
    fn secrets_of(settings: &Map<String, Value>, list: &str, identity: &str) -> Vec<String> {
        let mut accounts: Vec<String> = Self::records(settings, list)
            .iter()
            .filter_map(Self::read)
            .filter(|record| record.identity().as_deref() == Some(identity))
            .filter_map(|record| record.secret_account())
            .collect();
        accounts.sort();
        accounts.dedup();
        accounts
    }

    fn keep_default(&self, settings: &mut Map<String, Value>) {
        let Some(field) = self.default else { return };
        let records = Self::records(settings, self.list);
        let ids: Vec<&str> = records
            .iter()
            .filter_map(|record| record.get(self.id).and_then(Value::as_str))
            .collect();
        let current = settings.get(field).and_then(Value::as_str);
        if current.is_some_and(|id| ids.contains(&id)) {
            return;
        }
        let first = ids.first().map(|id| Value::String((*id).to_owned()));
        settings.insert(field.to_owned(), first.unwrap_or(Value::Null));
    }

    fn without(&self, settings: &mut Map<String, Value>, identity: &str) {
        let kept: Vec<Value> = Self::records(settings, self.list)
            .into_iter()
            .filter(|record| Self::identity_of(record).as_deref() != Some(identity))
            .collect();
        settings.insert(self.list.to_owned(), Value::Array(kept));
    }
}

impl<A: Listing> Book for Listed<A> {
    fn sign_ins(&self, data_dir: &Path) -> Result<Vec<SignIn>, String> {
        let settings = read_settings(&data_dir.join(self.file))?;
        let mut grouped: Vec<(String, String, Vec<Value>)> = Vec::new();
        for record in Self::records(&settings, self.list) {
            let Some(read) = Self::read(&record) else {
                continue;
            };
            let (Some(identity), Some(_)) = (read.identity(), read.secret_account()) else {
                continue;
            };
            match grouped.iter_mut().find(|(kept, _, _)| *kept == identity) {
                Some((_, _, records)) => records.push(record),
                None => grouped.push((identity, read.label(), vec![record])),
            }
        }
        let mut found = Vec::new();
        for (identity, label, records) in grouped {
            let mut secrets = Vec::new();
            for account in Self::secrets_of(&settings, self.list, &identity) {
                if let Some(secret) = read_raw(self.secrets, &account)? {
                    secrets.push(Kept {
                        service: self.secrets.service().to_owned(),
                        account,
                        secret,
                    });
                }
            }
            if secrets.is_empty() {
                continue;
            }
            found.push(SignIn {
                identity,
                label,
                secrets,
                records: Value::Array(records),
            });
        }
        Ok(found)
    }

    fn take_in(&self, data_dir: &Path, sign_in: &SignIn) -> Result<(), String> {
        let _turn = store::CHANGING.locked();
        let path = data_dir.join(self.file);
        let mut settings = read_settings(&path)?;
        let replaced = Self::secrets_of(&settings, self.list, &sign_in.identity);
        for kept in &sign_in.secrets {
            write_kept(kept)?;
        }
        for account in replaced {
            if !sign_in.secrets.iter().any(|kept| kept.account == account) {
                delete_raw(self.secrets.service(), &account)?;
            }
        }
        self.without(&mut settings, &sign_in.identity);
        let incoming = sign_in.records.as_array().cloned().unwrap_or_default();
        let incoming_ids: Vec<Value> = incoming
            .iter()
            .filter_map(|record| record.get(self.id).cloned())
            .collect();
        let mut records: Vec<Value> = Self::records(&settings, self.list)
            .into_iter()
            .filter(|record| {
                record
                    .get(self.id)
                    .is_none_or(|id| !incoming_ids.contains(id))
            })
            .collect();
        records.extend(incoming);
        settings.insert(self.list.to_owned(), Value::Array(records));
        self.keep_default(&mut settings);
        write_settings(&path, &settings)
    }

    fn let_go(&self, data_dir: &Path, identity: &str) -> Result<(), String> {
        let _turn = store::CHANGING.locked();
        let path = data_dir.join(self.file);
        let mut settings = read_settings(&path)?;
        let kept = Self::secrets_of(&settings, self.list, identity);
        self.without(&mut settings, identity);
        self.keep_default(&mut settings);
        write_settings(&path, &settings)?;
        for account in kept {
            delete_raw(self.secrets.service(), &account)?;
        }
        Ok(())
    }

    fn renewer(&self) -> Option<&'static dyn Renew> {
        self.renewer
    }
}

/// Settings that hold one sign-in in the whole file.
pub trait Whole: DeserializeOwned {
    /// None while nothing is signed in.
    fn identity(&self) -> Option<String>;
    fn label(&self) -> String;
    /// Every secret the sign-in keeps, by service and account.
    fn secrets(&self) -> Vec<(Secrets, String)>;
}

/// A plugin whose settings file is one sign-in. A sign-in taken in from the vault replaces it.
pub struct Single<C> {
    file: &'static str,
    settings: PhantomData<fn() -> C>,
}

impl<C: Whole> Single<C> {
    pub const fn new(file: &'static str) -> Self {
        Self {
            file,
            settings: PhantomData,
        }
    }

    fn read(&self, data_dir: &Path) -> Result<Option<(C, Value)>, String> {
        let settings = Value::Object(read_settings(&data_dir.join(self.file))?);
        let Ok(read) = serde_json::from_value::<C>(settings.clone()) else {
            return Ok(None);
        };
        Ok(Some((read, settings)))
    }
}

impl<C: Whole> Book for Single<C> {
    fn sign_ins(&self, data_dir: &Path) -> Result<Vec<SignIn>, String> {
        let Some((read, settings)) = self.read(data_dir)? else {
            return Ok(Vec::new());
        };
        let Some(identity) = read.identity() else {
            return Ok(Vec::new());
        };
        let mut secrets = Vec::new();
        for (service, account) in read.secrets() {
            if let Some(secret) = read_raw(service, &account)? {
                secrets.push(Kept {
                    service: service.service().to_owned(),
                    account,
                    secret,
                });
            }
        }
        if secrets.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![SignIn {
            identity,
            label: read.label(),
            secrets,
            records: settings,
        }])
    }

    fn take_in(&self, data_dir: &Path, sign_in: &SignIn) -> Result<(), String> {
        let _turn = store::CHANGING.locked();
        let Value::Object(settings) = &sign_in.records else {
            return Err("the vault's copy of these settings is not an object".into());
        };
        for kept in &sign_in.secrets {
            write_kept(kept)?;
        }
        write_settings(&data_dir.join(self.file), settings)
    }

    fn let_go(&self, data_dir: &Path, identity: &str) -> Result<(), String> {
        let _turn = store::CHANGING.locked();
        let Some((read, _)) = self.read(data_dir)? else {
            return Ok(());
        };
        if read.identity().as_deref() != Some(identity) {
            return Ok(());
        }
        for (service, account) in read.secrets() {
            delete_raw(service.service(), &account)?;
        }
        write_settings(&data_dir.join(self.file), &Map::new())
    }
}
