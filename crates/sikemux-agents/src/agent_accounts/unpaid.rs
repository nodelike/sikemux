//! Accounts whose provider refused a chat because the plan is unpaid. Each is kept off new
//! chats until a turn goes through on it again or it is signed in as someone else.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};

use sikemux_process::sync::Locked;

use super::key;
use super::sign_in::{signed_in, SignIn};
use crate::launch::account_environment;

/// Each unpaid account, with who it was signed in as when its plan was refused.
fn held() -> &'static Mutex<HashMap<String, Option<SignIn>>> {
    static HELD: OnceLock<Mutex<HashMap<String, Option<SignIn>>>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(HashMap::new()))
}

fn sign_in_of(provider: &str, config_path: Option<&str>) -> Option<SignIn> {
    let env: BTreeMap<String, String> = account_environment(provider, config_path)
        .into_iter()
        .collect();
    signed_in(provider, &env)
}

pub fn mark(provider: &str, config_path: Option<&str>) {
    let signed = sign_in_of(provider, config_path);
    held().locked().insert(key(provider, config_path), signed);
}

pub fn any() -> bool {
    !held().locked().is_empty()
}

/// Answers whether the account was held as unpaid.
pub fn clear(provider: &str, config_path: Option<&str>) -> bool {
    let mut held = held().locked();
    !held.is_empty() && held.remove(&key(provider, config_path)).is_some()
}

pub fn is_unpaid(provider: &str, config_path: Option<&str>) -> bool {
    let key = key(provider, config_path);
    let Some(refused) = held().locked().get(&key).cloned() else {
        return false;
    };
    if sign_in_of(provider, config_path) == refused {
        return true;
    }
    held().locked().remove(&key);
    false
}
