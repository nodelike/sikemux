//! What a start asks the person before it runs commands someone else wrote, and what their
//! yes grants.

use serde::{Deserialize, Serialize};

use crate::tasks::config::TrustCommand;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrustedConfig {
    pub path: String,
    pub fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrustedTask {
    pub project: String,
    pub cwd: String,
    pub command: String,
}

/// What a start asks the person before it runs commands someone else wrote.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrustAsk {
    pub title: String,
    pub body: String,
    pub commands: Vec<TrustCommand>,
    pub confirm_label: String,
    #[serde(skip)]
    pub grant: Grant,
}

#[derive(Clone, Debug)]
pub(crate) enum Grant {
    Config(TrustedConfig),
    Personal(TrustedTask),
}
