use std::fmt;
use std::io;

use sikemux_plugin_api::{FailureCategory, PluginError};

#[derive(Debug)]
pub enum AwsError {
    Aws(String),
    CliMissing(String),
    TokenExpired(String),
    NoCredentials(String),
    NoProfiles,
    ChooseProfile(Vec<String>),
    UnknownProfile(String, Vec<String>),
    NotFound(String),
    BadArg(&'static str),
    Io(io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for AwsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Aws(message) => write!(formatter, "aws: {message}"),
            Self::CliMissing(bin) => write!(
                formatter,
                "The AWS CLI is not installed ({bin} is not on PATH). Ask the person to install it."
            ),
            Self::TokenExpired(profile) => {
                write!(formatter, "the sign-in for profile {profile} expired")
            }
            Self::NoCredentials(profile) => {
                write!(formatter, "profile {profile} has no credentials")
            }
            Self::NoProfiles => formatter.write_str(
                "No AWS profile is set up. Ask the person to set one up in ~/.aws/config.",
            ),
            Self::ChooseProfile(profiles) => write!(
                formatter,
                "Say which AWS profile to use: {}.",
                profile_list(profiles)
            ),
            Self::UnknownProfile(profile, profiles) => write!(
                formatter,
                "There is no AWS profile {profile}. Profiles: {}.",
                profile_list(profiles)
            ),
            Self::NotFound(message) => formatter.write_str(message),
            Self::BadArg(message) => write!(formatter, "invalid argument: {message}"),
            Self::Io(error) => write!(formatter, "io: {error}"),
            Self::Json(error) => write!(formatter, "json: {error}"),
        }
    }
}

const LISTED_PROFILES: usize = 40;

fn profile_list(profiles: &[String]) -> String {
    let mut text = profiles
        .iter()
        .take(LISTED_PROFILES)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if profiles.len() > LISTED_PROFILES {
        text.push_str(&format!(" and {} more", profiles.len() - LISTED_PROFILES));
    }
    text
}

impl From<io::Error> for AwsError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for AwsError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<AwsError> for PluginError {
    fn from(error: AwsError) -> Self {
        let category = match &error {
            AwsError::Aws(_)
            | AwsError::Io(_)
            | AwsError::Json(_)
            | AwsError::CliMissing(_)
            | AwsError::NoProfiles => FailureCategory::own("aws"),
            AwsError::TokenExpired(_) | AwsError::NoCredentials(_) => FailureCategory::Auth,
            AwsError::NotFound(_) => FailureCategory::NotFound,
            AwsError::ChooseProfile(_) | AwsError::UnknownProfile(..) | AwsError::BadArg(_) => {
                FailureCategory::BadParams
            }
        };
        PluginError::new(category, error.to_string())
    }
}

pub type AwsResult<T> = Result<T, AwsError>;
