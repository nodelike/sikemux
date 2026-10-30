//! A link that shows its own address, to a pull request or issue on a code host, reads as
//! GitHub writes a reference to one: `owner/repo#12`, or `group/project!12` for a GitLab
//! merge request.

use crate::{Element, Node};

pub fn fold(node: Node) -> Node {
    let Node::Element(Element::A { href, title, c }) = node else {
        return node;
    };
    let named = match c.as_slice() {
        [Node::Text(text)] if *text == href => name(&href),
        _ => None,
    };
    let c = named.map_or(c, |name| vec![Node::Text(name)]);
    Node::Element(Element::A { href, title, c })
}

/// How GitHub writes a reference to the pull request or issue at `href`, when it is one.
pub fn name(href: &str) -> Option<String> {
    let rest = href
        .strip_prefix("https://")
        .or_else(|| href.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    let path = path.strip_suffix('/').unwrap_or(path);
    if path.contains(['?', '#']) {
        return None;
    }
    let parts: Vec<&str> = path.split('/').collect();
    let [place @ .., kind, number] = parts.as_slice() else {
        return None;
    };
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let (repository, mark) = match (place, *kind) {
        ([project @ .., "-"], "merge_requests") => (project, '!'),
        ([project @ .., "-"], "issues") => (project, '#'),
        ([_, _], "pull") if host == "github.com" => (place, '#'),
        ([_, _], "pull-requests") if host == "bitbucket.org" => (place, '#'),
        ([_, _], "issues") if matches!(host, "github.com" | "bitbucket.org") => (place, '#'),
        _ => return None,
    };
    if repository.is_empty() || repository.iter().any(|part| part.is_empty()) {
        return None;
    }
    Some(format!("{}{mark}{number}", repository.join("/")))
}
