use std::fmt;
use std::io;

use sikemux_plugin_api::PluginError;

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
                "The AWS CLI is not installed ({bin} is not on PATH). Ask the person to install it from the AWS pane in Sikemux."
            ),
            Self::TokenExpired(profile) => write!(
                formatter,
                "AWS sign-in expired for profile {profile}. Ask the person to sign in from the AWS pane in Sikemux; it needs their browser, so you cannot do it."
            ),
            Self::NoCredentials(profile) => write!(
                formatter,
                "Profile {profile} has no AWS credentials. Ask the person to sign in or fix the profile from the AWS pane in Sikemux."
            ),
            Self::NoProfiles => formatter.write_str(
                "No AWS profile is set up. Ask the person to set one up from the AWS pane in Sikemux.",
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
            AwsError::Aws(_) | AwsError::Io(_) | AwsError::Json(_) => "aws",
            AwsError::CliMissing(_) => "aws-cli-missing",
            AwsError::TokenExpired(_) => "aws-token-expired",
            AwsError::NoCredentials(_) => "aws-no-credentials",
            AwsError::NoProfiles => "aws-no-profiles",
            AwsError::NotFound(_) => "not-found",
            AwsError::ChooseProfile(_) | AwsError::UnknownProfile(..) | AwsError::BadArg(_) => {
                "bad-params"
            }
        };
        PluginError::new(category, error.to_string())
    }
}

pub type AwsResult<T> = Result<T, AwsError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_are_the_ones_the_frontend_reads() {
        let category = |error: AwsError| PluginError::from(error).category;
        assert_eq!(category(AwsError::Aws("x".into())), "aws");
        assert_eq!(
            category(AwsError::CliMissing("aws".into())),
            "aws-cli-missing"
        );
        assert_eq!(
            category(AwsError::TokenExpired("p".into())),
            "aws-token-expired"
        );
        assert_eq!(
            category(AwsError::NoCredentials("p".into())),
            "aws-no-credentials"
        );
        assert_eq!(category(AwsError::BadArg("x")), "bad-params");
    }
}
