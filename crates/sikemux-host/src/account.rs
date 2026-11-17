//! A host with no browser signs in by a code its person approves on a device
//! already signed in. The core runs the sign-in; this only shows the code.

use serde_json::json;
use sikemux_client::client::CoreClient;
use sikemux_core::server::account::SignInCode;

/// Signs this host in without a browser. `show` gets the code its person
/// approves on a device already signed in. Answers with the account.
pub async fn sign_in_by_code(
    core: &CoreClient,
    show: impl FnOnce(&SignInCode),
) -> Result<String, String> {
    let started = core
        .call("accountSignInByCode", json!({}))
        .await
        .map_err(|error| error.to_string())?;
    let code: SignInCode = serde_json::from_value(started).map_err(|error| error.to_string())?;
    show(&code);
    core.call("accountSignInFinish", json!({}))
        .await
        .map_err(|error| error.to_string())?;
    core.remote_status()
        .await
        .map_err(|error| error.to_string())?
        .owner
        .ok_or_else(|| "the account let this host go as it signed in".to_owned())
}
