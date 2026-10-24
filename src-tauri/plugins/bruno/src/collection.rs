// A Bruno collection on disk: a folder with a `bruno.json` or `collection.bru`, its requests in
// folders below, and its environments in `environments/`. A workspace folder may hold several
// collections; each request belongs to the nearest one above it, and so do its environments.
// What the pane changes is written here too, and only inside the folder it opened.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sikemux_plugin_api::bruno::{
    Collection, Env, KnownWorkspace, KnownWorkspaces, Request, RequestView, Scope, TreeNode,
};

use crate::bru;
use crate::error::{BrunoError, BrunoResult};

const ENVIRONMENTS: &str = "environments";
const SKIPPED: [&str; 3] = [".git", "node_modules", ENVIRONMENTS];
/// Where a request or folder whose meta gives no `seq` goes: after those that do.
const LAST: i64 = i64::MAX;

struct Entry {
    name: String,
    path: PathBuf,
    folder: bool,
}

fn entries(dir: &Path) -> BrunoResult<Vec<Entry>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        found.push(Entry {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: entry.path(),
            folder: entry.file_type()?.is_dir(),
        });
    }
    Ok(found)
}

fn is_collection(entries: &[Entry]) -> bool {
    entries.iter().any(|entry| {
        !entry.folder && matches!(entry.name.as_str(), "bruno.json" | "collection.bru")
    })
}

fn text(path: &Path) -> String {
    path.display().to_string()
}

fn base_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| text(path), |name| name.to_string_lossy().into_owned())
}

fn stem(name: &str) -> &str {
    name.strip_suffix(".bru").unwrap_or(name)
}

/// The folder's scope file: `folder.bru`, or a nested collection's `collection.bru`.
fn scope_file(entries: &[Entry]) -> Option<&Entry> {
    entries.iter().find(|entry| {
        !entry.folder && matches!(entry.name.as_str(), "folder.bru" | "collection.bru")
    })
}

fn tree(entries: Vec<Entry>, collection: &str) -> BrunoResult<Vec<(i64, TreeNode)>> {
    let mut nodes = Vec::new();
    for entry in entries {
        if let Some(node) = node(entry, collection)? {
            nodes.push(node);
        }
    }
    nodes.sort_by(|(a_seq, a), (b_seq, b)| {
        a_seq.cmp(b_seq).then_with(|| name_of(a).cmp(name_of(b)))
    });
    Ok(nodes)
}

fn name_of(node: &TreeNode) -> &str {
    match node {
        TreeNode::Request { name, .. } | TreeNode::Folder { name, .. } => name,
    }
}

fn node(entry: Entry, collection: &str) -> BrunoResult<Option<(i64, TreeNode)>> {
    if entry.folder {
        if SKIPPED.contains(&entry.name.as_str()) {
            return Ok(None);
        }
        let children = entries(&entry.path)?;
        let scope = match scope_file(&children) {
            Some(file) => Some(bru::scope(&fs::read_to_string(&file.path)?, &entry.name)),
            None => None,
        };
        let inner = match is_collection(&children) {
            true => text(&entry.path),
            false => collection.to_owned(),
        };
        let children = tree(children, &inner)?;
        if children.is_empty() && scope.is_none() {
            return Ok(None);
        }
        let seq = scope
            .as_ref()
            .and_then(|scope| scope.meta.seq)
            .unwrap_or(LAST);
        let name = scope
            .map(|scope| scope.meta.name)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| entry.name.clone());
        return Ok(Some((
            seq,
            TreeNode::Folder {
                name,
                path: text(&entry.path),
                children: children.into_iter().map(|(_, node)| node).collect(),
            },
        )));
    }
    if !entry.name.ends_with(".bru")
        || matches!(entry.name.as_str(), "collection.bru" | "folder.bru")
    {
        return Ok(None);
    }
    let request = bru::request(&fs::read_to_string(&entry.path)?);
    let name = match request.meta.name.is_empty() {
        true => stem(&entry.name).to_owned(),
        false => request.meta.name.clone(),
    };
    Ok(Some((
        request.meta.seq.unwrap_or(LAST),
        TreeNode::Request {
            name,
            path: text(&entry.path),
            method: request.method,
            collection_path: collection.to_owned(),
        },
    )))
}

/// Every environment below `dir`, each tagged with the collection it belongs to.
fn envs(dir: &Path, listed: &[Entry], collection: &str) -> BrunoResult<Vec<Env>> {
    let here = match is_collection(listed) {
        true => text(dir),
        false => collection.to_owned(),
    };
    let mut found = Vec::new();
    for entry in listed.iter().filter(|entry| entry.folder) {
        if entry.name == ENVIRONMENTS {
            for file in entries(&entry.path)?
                .into_iter()
                .filter(|file| !file.folder && file.name.ends_with(".bru"))
            {
                let owner = if here.is_empty() {
                    dir.to_path_buf()
                } else {
                    PathBuf::from(&here)
                };
                found.push(bru::env(
                    &fs::read_to_string(&file.path)?,
                    stem(&file.name),
                    &here,
                    &base_name(&owner),
                ));
            }
        } else if !SKIPPED.contains(&entry.name.as_str()) {
            found.extend(envs(&entry.path, &entries(&entry.path)?, &here)?);
        }
    }
    Ok(found)
}

#[derive(Deserialize)]
pub struct CollectionAsk {
    pub root: String,
}

pub fn collection(ask: &CollectionAsk) -> BrunoResult<Collection> {
    let root = Path::new(&ask.root);
    let entries = entries(root)?;
    let config = match entries
        .iter()
        .find(|entry| !entry.folder && entry.name == "collection.bru")
    {
        Some(file) => Some(bru::scope(
            &fs::read_to_string(&file.path)?,
            &base_name(root),
        )),
        None => None,
    };
    let collection = match is_collection(&entries) {
        true => ask.root.clone(),
        false => String::new(),
    };
    let mut found = envs(root, &entries, &collection)?;
    found.sort_by(|a, b| {
        a.collection_name
            .cmp(&b.collection_name)
            .then_with(|| a.name.cmp(&b.name))
    });
    let name = config
        .map(|config| config.meta.name)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| base_name(root));
    Ok(Collection {
        root_path: ask.root.clone(),
        name,
        envs: found,
        tree: tree(entries, &collection)?
            .into_iter()
            .map(|(_, node)| node)
            .collect(),
    })
}

#[derive(Deserialize, Default)]
pub struct WorkspacesAsk {
    /// The folders opened before, most recent first.
    #[serde(default)]
    pub known: Vec<String>,
    /// The folder open now.
    #[serde(default)]
    pub loaded: String,
}

/// The folders opened before, each named by its `bruno.json` or `collection.bru` when it has
/// one, else by the folder; the one open now is left out.
pub fn workspaces(ask: &WorkspacesAsk) -> BrunoResult<KnownWorkspaces> {
    let named = |path: &str| -> String {
        let folder = Path::new(path);
        let from_json = fs::read_to_string(folder.join("bruno.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|config| {
                config
                    .get("name")
                    .and_then(|name| name.as_str())
                    .map(str::to_owned)
            });
        let from_scope = || {
            fs::read_to_string(folder.join("collection.bru"))
                .ok()
                .map(|text| bru::scope(&text, "").meta.name)
        };
        from_json
            .or_else(from_scope)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| base_name(folder))
    };
    Ok(KnownWorkspaces {
        workspaces: ask
            .known
            .iter()
            .filter(|path| **path != ask.loaded)
            .map(|path| KnownWorkspace {
                name: named(path),
                path: path.clone(),
            })
            .collect(),
    })
}

#[derive(Deserialize)]
pub struct RequestAsk {
    pub root: String,
    pub path: String,
}

/// The request at `path`, with the collection's scope and each folder's above it.
pub fn request(ask: &RequestAsk) -> BrunoResult<RequestView> {
    inside(&ask.root, &ask.path)?;
    let root = PathBuf::from(&ask.root);
    let file = PathBuf::from(&ask.path);
    let request = bru::request(&fs::read_to_string(&file)?);
    let mut scopes: Vec<Scope> = Vec::new();
    let mut collection = String::new();
    let mut dir = file.parent().map(Path::to_path_buf);
    let mut above: Vec<(PathBuf, Vec<Entry>)> = Vec::new();
    while let Some(here) = dir {
        if !here.starts_with(&root) {
            break;
        }
        let listed = entries(&here)?;
        let next = here.parent().map(Path::to_path_buf);
        above.push((here, listed));
        dir = next;
    }
    for (here, listed) in above.iter().rev() {
        if is_collection(listed) {
            collection = text(here);
        }
        if let Some(file) = scope_file(listed) {
            scopes.push(bru::scope(
                &fs::read_to_string(&file.path)?,
                &base_name(here),
            ));
        }
    }
    Ok(RequestView {
        request,
        collection_path: collection,
        scopes,
    })
}

/// Makes sure `path` lies inside the folder the pane opened.
pub fn check_inside(root: &str, path: &str) -> BrunoResult<()> {
    inside(root, path).map(|_| ())
}

/// `path`, made sure to lie inside the folder the pane opened, and that folder.
fn inside(root: &str, path: &str) -> BrunoResult<PathBuf> {
    let root = fs::canonicalize(root)?;
    let target = Path::new(path);
    let parent = target
        .parent()
        .ok_or(BrunoError::BadArg("the path has no folder"))?;
    let parent = fs::canonicalize(parent)?;
    if !parent.starts_with(&root) {
        return Err(BrunoError::BadArg("the path is outside the collection"));
    }
    Ok(root)
}

fn safe_file_name(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect()
}

/// Writes `contents` to `path`, which must not exist yet.
fn write_new(path: &Path, contents: &str) -> BrunoResult<()> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(contents.as_bytes())?;
    Ok(())
}

#[derive(Deserialize)]
pub struct SaveAsk {
    pub root: String,
    pub path: String,
    pub request: Request,
}

pub fn save(ask: SaveAsk) -> BrunoResult<()> {
    inside(&ask.root, &ask.path)?;
    fs::write(&ask.path, bru::write_request(&ask.request))?;
    Ok(())
}

#[derive(Deserialize)]
pub struct NewAsk {
    pub root: String,
    /// The folder it goes in.
    pub folder: String,
    pub name: String,
}

/// Makes a request named `name` in the folder, and says where it is.
pub fn new_request(ask: NewAsk) -> BrunoResult<String> {
    let name = ask.name.trim();
    if name.is_empty() {
        return Err(BrunoError::BadArg("a name is needed"));
    }
    let path = Path::new(&ask.folder).join(format!("{}.bru", safe_file_name(name)));
    inside(&ask.root, &text(&path))?;
    write_new(&path, &bru::write_request(&bru::new_request(name)))?;
    Ok(text(&path))
}

pub fn new_folder(ask: NewAsk) -> BrunoResult<String> {
    let name = ask.name.trim();
    if name.is_empty() {
        return Err(BrunoError::BadArg("a name is needed"));
    }
    let folder = Path::new(&ask.folder).join(safe_file_name(name));
    inside(&ask.root, &text(&folder))?;
    fs::create_dir(&folder)?;
    write_new(
        &folder.join("folder.bru"),
        &format!("meta {{\n  name: {name}\n}}\n"),
    )?;
    Ok(text(&folder))
}

#[derive(Deserialize)]
pub struct RenameAsk {
    pub root: String,
    pub path: String,
    pub name: String,
    /// The request as edited, when it has changes not saved yet.
    #[serde(default)]
    pub request: Option<Request>,
}

/// Renames a request, its name and its file to match, and says where it is now.
pub fn rename(ask: RenameAsk) -> BrunoResult<String> {
    let name = ask.name.trim();
    if name.is_empty() {
        return Err(BrunoError::BadArg("a name is needed"));
    }
    inside(&ask.root, &ask.path)?;
    let path = PathBuf::from(&ask.path);
    let mut request = match ask.request {
        Some(request) => request,
        None => bru::request(&fs::read_to_string(&path)?),
    };
    request.meta.name = name.to_owned();
    let renamed = path
        .parent()
        .unwrap_or(Path::new(""))
        .join(format!("{}.bru", safe_file_name(name)));
    if renamed == path {
        fs::write(&path, bru::write_request(&request))?;
    } else {
        write_new(&renamed, &bru::write_request(&request))?;
        fs::remove_file(&path)?;
    }
    Ok(text(&renamed))
}

#[derive(Deserialize)]
pub struct DeleteAsk {
    pub root: String,
    pub path: String,
}

pub fn delete(ask: DeleteAsk) -> BrunoResult<()> {
    let root = inside(&ask.root, &ask.path)?;
    let path = fs::canonicalize(&ask.path)?;
    if path == root {
        return Err(BrunoError::BadArg(
            "the collection itself cannot be deleted here",
        ));
    }
    match path.is_dir() {
        true => fs::remove_dir_all(&path)?,
        false => fs::remove_file(&path)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn loads_a_workspace_of_collections_with_their_environments_and_folder_scopes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let api = root.join("collections/api");
        write(&api.join("bruno.json"), "{}");
        write(
            &api.join("environments/dev.bru"),
            "vars {\n  baseUrl: http://localhost\n}\n",
        );
        write(
            &api.join("users/folder.bru"),
            "meta {\n  name: Users\n  seq: 1\n}\nvars {\n  part: users\n}\n",
        );
        write(
            &api.join("users/list.bru"),
            "meta {\n  name: List users\n  seq: 2\n}\nget {\n  url: {{baseUrl}}/users\n}\n",
        );
        write(
            &api.join("users/create.bru"),
            "meta {\n  name: Create user\n  seq: 1\n}\npost {\n  url: /users\n}\n",
        );
        fs::create_dir_all(root.join("node_modules/x")).unwrap();

        let loaded = collection(&CollectionAsk { root: text(&root) }).unwrap();
        let api_path = text(&api);
        assert_eq!(loaded.envs.len(), 1);
        assert_eq!(loaded.envs[0].id, format!("{api_path}::dev"));
        assert_eq!(loaded.envs[0].collection_name, "api");
        let TreeNode::Folder { children, .. } = &loaded.tree[0] else {
            panic!("not a folder")
        };
        let TreeNode::Folder { name, children, .. } = &children[0] else {
            panic!("not a folder")
        };
        assert_eq!(name, "api");
        let TreeNode::Folder { name, children, .. } = &children[0] else {
            panic!("not a folder")
        };
        assert_eq!(name, "Users");
        let names: Vec<&str> = children.iter().map(name_of).collect();
        assert_eq!(names, ["Create user", "List users"]);
        let TreeNode::Request {
            collection_path, ..
        } = &children[0]
        else {
            panic!("not a request")
        };
        assert_eq!(collection_path, &api_path);

        let view = request(&RequestAsk {
            root: text(&root),
            path: text(&api.join("users/list.bru")),
        })
        .unwrap();
        assert_eq!(view.collection_path, api_path);
        assert_eq!(view.scopes.len(), 1);
        assert_eq!(view.scopes[0].meta.name, "Users");
    }

    #[test]
    fn writes_only_inside_the_folder_it_opened() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("collection");
        fs::create_dir_all(&root).unwrap();
        let made = new_request(NewAsk {
            root: text(&root),
            folder: text(&root),
            name: "Ping / pong".into(),
        })
        .unwrap();
        assert!(made.ends_with("Ping _ pong.bru"));
        let outside = new_request(NewAsk {
            root: text(&root),
            folder: text(dir.path()),
            name: "Escape".into(),
        });
        assert!(outside.is_err());
        let renamed = rename(RenameAsk {
            root: text(&root),
            path: made,
            name: "Pong".into(),
            request: None,
        })
        .unwrap();
        assert_eq!(
            bru::request(&fs::read_to_string(&renamed).unwrap())
                .meta
                .name,
            "Pong"
        );
        assert!(delete(DeleteAsk {
            root: text(&root),
            path: text(&root),
        })
        .is_err());
    }
}
