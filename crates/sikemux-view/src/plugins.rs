//! A plugin as clients see it: how it is listed and how a person signs in to it.

use serde::{Deserialize, Serialize};

/// The form a person signs in to the plugin's service with by pasting a token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TokenForm {
    /// What the form calls with what was typed, under each field's name.
    pub method: String,
    /// What signing in brings, said above the form.
    pub lede: String,
    pub fields: Vec<FormField>,
    /// Where a token is made, `{server}` standing for the server typed in the
    /// `host` field, or `default_server` while it is empty.
    #[serde(default)]
    pub token_page: Option<String>,
    #[serde(default)]
    pub default_server: Option<String>,
    /// Boxes to tick, sent as true or false under each one's name whichever way is used.
    #[serde(default)]
    pub checks: Vec<FormCheck>,
    /// A second way in beside the token, such as a password.
    #[serde(default)]
    pub other_way: Option<OtherWay>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormField {
    pub name: String,
    pub label: String,
    pub placeholder: String,
    #[serde(default)]
    pub secret: bool,
    /// Sent just as typed, spaces at its ends included, as a password must be.
    #[serde(default)]
    pub exact: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormCheck {
    pub name: String,
    pub label: String,
}

/// A form's second way in. A field named as one of the form's own keeps what was typed in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OtherWay {
    pub method: String,
    /// The link that switches to this way.
    pub offer: String,
    /// The link that switches back.
    pub back: String,
    pub fields: Vec<FormField>,
}

impl TokenForm {
    /// The method to call and the fields to show, for the second way in or the first.
    pub fn way(&self, other: bool) -> (&str, &[FormField]) {
        match self.other_way.as_ref().filter(|_| other) {
            Some(way) => (&way.method, &way.fields),
            None => (&self.method, &self.fields),
        }
    }

    /// Where a token is made for the server typed in, if the service says.
    pub fn token_page(&self, server: &str) -> Option<String> {
        let server = server.trim().trim_start_matches("https://");
        let server = server.split('/').next().unwrap_or_default();
        let server = match server {
            "" => self.default_server.as_deref().unwrap_or_default(),
            typed => typed,
        };
        Some(self.token_page.as_ref()?.replace("{server}", server))
    }
}

/// A plugin as clients list it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// Whether its sign-in works; none for a plugin with no sign-in, or one
    /// switched off.
    pub signed_in: Option<bool>,
    /// Who is signed in, as its status names them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_in_as: Option<String>,
    /// Why its sign-in does not work, as the service said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    /// It can be signed in to through the browser in this build.
    #[serde(default)]
    pub browser_sign_in: bool,
    /// It can be signed out of.
    #[serde(default)]
    pub sign_out: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_form: Option<TokenForm>,
}
