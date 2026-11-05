//! Which of a plugin's accounts a call is for. A call names it once, and
//! everything the call does on the way reads it from here.

use std::future::Future;

use serde_json::Value;

tokio::task_local! {
    static CHOSEN: Option<String>;
}

/// Runs `work` as the account named, or with none named, as the default one.
pub fn as_account<F: Future>(account: Option<String>, work: F) -> impl Future<Output = F::Output> {
    CHOSEN.scope(account, work)
}

pub fn chosen() -> Option<String> {
    CHOSEN.try_with(Clone::clone).ok().flatten()
}

/// Which account a call's arguments name; with none named, the default one.
pub fn named_in(input: &Value) -> Option<String> {
    input
        .get("account")
        .and_then(Value::as_str)
        .filter(|account| !account.is_empty())
        .map(str::to_string)
}
