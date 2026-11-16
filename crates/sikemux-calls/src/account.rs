use serde::{Deserialize, Serialize};

#[derive(serde::Deserialize, serde::Serialize)]
pub struct DeviceParams {
    pub key: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct AccountPictureParams {
    pub hash: String,
}

/// Where the browser goes to sign in.
#[derive(Serialize, Deserialize)]
pub struct SignInPage {
    pub url: String,
}

/// What a host with no browser shows its person: the code they enter on a device
/// already signed in, and where.
#[derive(Serialize, Deserialize)]
pub struct SignInCode {
    pub url: String,
    pub code: String,
}
