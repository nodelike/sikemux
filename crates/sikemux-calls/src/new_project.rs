use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct NewProjectParams {
    pub name: String,
}

#[derive(Deserialize, Serialize)]
pub struct CloneProjectParams {
    pub url: String,
}

/// The repository's own name in a git address, `app` for `git@host:team/app.git`; none for
/// text that is not one.
pub fn repo_name(url: &str) -> Option<String> {
    let url = url.trim();
    let scp = url
        .split_once(':')
        .is_some_and(|(host, path)| host.contains('@') && !host.contains('/') && !path.is_empty());
    let schemed = ["https://", "http://", "ssh://", "git://", "file://"]
        .iter()
        .any(|scheme| url.starts_with(scheme));
    if !(scp || schemed) || url.contains(char::is_whitespace) {
        return None;
    }
    let last = url.trim_end_matches('/').rsplit(['/', ':']).next()?;
    let name: String = (last.trim_end_matches(".git").chars())
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .collect();
    let name = name.trim_matches('.');
    (!name.is_empty()).then(|| name.to_owned())
}
