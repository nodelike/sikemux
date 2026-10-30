//! What the Bruno plugin's views hand a pane: a collection's requests and environments, and a
//! request as its `.bru` file says it, with the folders and collection above it.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum HttpMethod {
    #[default]
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Options,
    Head,
}

/// A row of headers, parameters, form fields or variables; a disabled one is written with `~`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct KeyVal {
    pub name: String,
    pub value: String,
    pub enabled: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum BodyMode {
    #[default]
    None,
    Json,
    Text,
    Xml,
    Sparql,
    Graphql,
    FormUrlencoded,
    MultipartForm,
    File,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GraphqlBody {
    pub query: String,
    pub variables: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Body {
    pub mode: BodyMode,
    /// The text of a JSON, text, XML, SPARQL or GraphQL body.
    pub text: String,
    pub graphql: GraphqlBody,
    /// The rows of a form body; a multipart value may be `@file(/path)`.
    pub form: Vec<KeyVal>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    None,
    #[default]
    Inherit,
    Bearer,
    Basic,
    Apikey,
    Awsv4,
    Digest,
    Wsse,
    Oauth2,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum KeyPlacement {
    #[default]
    Header,
    Queryparams,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct BearerAuth {
    pub token: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct BasicAuth {
    pub username: String,
    pub password: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct KeyAuth {
    pub key: String,
    pub value: String,
    pub placement: KeyPlacement,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Auth {
    pub mode: AuthMode,
    pub bearer: BearerAuth,
    pub basic: BasicAuth,
    pub apikey: KeyAuth,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub seq: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct RequestSettings {
    pub encode_url: Option<bool>,
    pub timeout: Option<u64>,
}

/// A block the model does not fold in, kept as written so saving gives it back unchanged.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct RawBlock {
    pub name: String,
    /// `{` or `[`.
    pub delim: String,
    pub raw: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Params {
    pub query: Vec<KeyVal>,
    pub path: Vec<KeyVal>,
}

/// Variables set before a request goes, and those taken from its response.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Vars {
    pub req: Vec<KeyVal>,
    pub res: Vec<KeyVal>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Scripts {
    pub pre: String,
    pub post: String,
}

/// A `.bru` request file.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub meta: Meta,
    pub method: HttpMethod,
    pub url: String,
    pub params: Params,
    pub headers: Vec<KeyVal>,
    pub body: Body,
    pub auth: Auth,
    pub vars: Vars,
    pub scripts: Scripts,
    pub assertions: Vec<KeyVal>,
    pub tests: String,
    pub docs: String,
    pub settings: RequestSettings,
    pub extra: Vec<RawBlock>,
}

/// What a collection or a folder shares with the requests under it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    pub meta: Meta,
    pub auth: Auth,
    pub headers: Vec<KeyVal>,
    pub vars: Vars,
    pub scripts: Scripts,
    pub extra: Vec<RawBlock>,
}

/// An environment, from `<collection>/environments/<name>.bru`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Env {
    /// Unique across the workspace: `<collection>::<name>`.
    pub id: String,
    pub name: String,
    /// The collection it belongs to, by its folder.
    pub collection_path: String,
    pub collection_name: String,
    pub vars: Vec<KeyVal>,
    /// The variables whose values are typed in each session rather than kept in the file.
    pub secret_names: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum TreeNode {
    Request {
        name: String,
        path: String,
        method: HttpMethod,
        /// The collection it belongs to, by its folder, for its environments.
        collection_path: String,
    },
    Folder {
        name: String,
        path: String,
        children: Vec<TreeNode>,
    },
}

/// A collection, or a workspace of them: every request in the order the files give, and every
/// environment.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub root_path: String,
    pub name: String,
    pub envs: Vec<Env>,
    pub tree: Vec<TreeNode>,
}

/// One request as its file says it, with what it inherits.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RequestView {
    pub request: Request,
    /// The collection it belongs to, by its folder.
    pub collection_path: String,
    /// The collection's scope, then each folder's above the request, outermost first.
    pub scopes: Vec<Scope>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    pub name: String,
    pub value: String,
}

/// What came back from a request.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<Header>,
    pub body: String,
    pub is_binary: bool,
    pub size_bytes: u64,
    pub duration_ms: u64,
}

/// The request as it went, after scripts, variables and auth.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Sent {
    pub method: String,
    pub url: String,
    pub headers: Vec<Header>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScriptLog {
    /// `log`, `info`, `warn`, `error` or `debug`.
    pub level: String,
    pub text: String,
}

/// A script's `test` or an `assert` row, and how it went.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub name: String,
    pub passed: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// One run of a request: what went, what came back or why nothing did, and what its scripts
/// logged, tested and set.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunResult {
    pub response: Option<Response>,
    pub error: Option<String>,
    pub sent: Sent,
    pub logs: Vec<ScriptLog>,
    pub tests: Vec<Check>,
    /// Variables the run's scripts changed, for the session to keep for later runs.
    pub env_updates: std::collections::BTreeMap<String, String>,
}

/// A collection folder the person opened before, named as Bruno names it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnownWorkspace {
    pub path: String,
    pub name: String,
}

/// The folders opened before, the one open now left out.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnownWorkspaces {
    pub workspaces: Vec<KnownWorkspace>,
}
