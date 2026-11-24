// A job's whole definition, as Rundeck exports it, and changing it: put back whole, or only
// the parts named, against what Rundeck holds now.

use std::hash::{DefaultHasher, Hash, Hasher};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_plugin_api::cut;

use crate::agent::{self, resolve};
use crate::client::{
    get_json_at_version, post_json_at_version, seg, Session, JOB_DEFINITION_API_VERSION,
};
use crate::error::{RundeckError, RundeckResult};
use crate::jobs::JobDetail;
use crate::shape::is_production;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportAsk {
    job: String,
    project: Option<String>,
}

#[derive(Serialize)]
pub struct Exported {
    project: String,
    path: String,
    /// What this definition reads as, for `edit` to know it is still the one that was read.
    revision: String,
    definition: Value,
}

pub async fn export(session: &Session, ask: ExportAsk) -> RundeckResult<Exported> {
    let detail = resolve(session, &ask.job, ask.project.as_deref()).await?;
    let definition = live(session, &detail).await?;
    Ok(Exported {
        path: agent::path_of(&detail),
        project: detail.project,
        revision: revision(&definition),
        definition,
    })
}

async fn live(session: &Session, detail: &JobDetail) -> RundeckResult<Value> {
    let definition: Value = get_json_at_version(
        session,
        JOB_DEFINITION_API_VERSION,
        &format!("/job/{}", seg(&detail.id)?),
        &[("format", "json".into())],
    )
    .await?;
    single(definition)
}

fn revision(definition: &Value) -> String {
    let mut sorted = definition.clone();
    sorted.sort_all_objects();
    let mut hasher = DefaultHasher::new();
    sorted.to_string().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportAsk {
    job: String,
    project: Option<String>,
    definition: Value,
    #[serde(default)]
    production: bool,
}

#[derive(Debug, Serialize)]
pub struct Imported {
    project: String,
    path: String,
    id: String,
}

fn single(definition: Value) -> RundeckResult<Value> {
    match definition {
        Value::Array(mut items) if items.len() == 1 => Ok(items.remove(0)),
        Value::Array(_) => Err(RundeckError::BadArg(
            "a definition holds exactly one job".into(),
        )),
        Value::Object(_) => Ok(definition),
        _ => Err(RundeckError::BadArg(
            "definition is the object rundeck export answered with".into(),
        )),
    }
}

/// The group a definition puts its job in.
pub fn group_of(definition: &Value) -> Option<&str> {
    let job = match definition {
        Value::Array(items) => items.first()?,
        job => job,
    };
    job.get("group").and_then(Value::as_str)
}

fn texts<'a>(value: &'a Value, found: &mut Vec<&'a str>) {
    match value {
        Value::String(text) => found.push(text),
        Value::Array(items) => items.iter().for_each(|item| texts(item, found)),
        Value::Object(fields) => fields.values().for_each(|field| texts(field, found)),
        _ => {}
    }
}

/// Refuses a definition that holds only part of what it would replace.
fn check_whole(definition: &Value, live: &Value) -> RundeckResult<()> {
    let (mut now, mut then) = (Vec::new(), Vec::new());
    texts(live, &mut now);
    texts(definition, &mut then);
    let cut = cut::marked(definition)
        || definition.get("omitted").is_some()
        || then
            .iter()
            .any(|text| !now.contains(text) && now.iter().any(|whole| cut::cut_of(text, whole)));
    if cut {
        return Err(RundeckError::BadArg(
            "nothing was changed: this definition was cut short when it was read, and the job would lose the rest. Change only the parts you mean to with rundeck edit, or send the whole definition from the file export's omitted names."
                .into(),
        ));
    }
    Ok(())
}

fn check_production(
    detail: &JobDetail,
    definition: &Value,
    agreed: bool,
    prod_envs: &[String],
) -> RundeckResult<()> {
    let production = is_production(&detail.project, detail.group.as_deref(), prod_envs)
        || is_production(&detail.project, group_of(definition), prod_envs);
    if production && !agreed {
        return Err(RundeckError::BadArg(format!(
            "{} is a production job. Ask the person first, and call again with production: true once they agree.",
            agent::path_of(detail)
        )));
    }
    Ok(())
}

pub async fn import(
    session: &Session,
    ask: ImportAsk,
    prod_envs: &[String],
) -> RundeckResult<Imported> {
    let detail = resolve(session, &ask.job, ask.project.as_deref()).await?;
    let definition = single(ask.definition)?;
    let named = ["uuid", "id"]
        .iter()
        .find_map(|key| definition.get(*key).and_then(Value::as_str));
    if named.is_some_and(|id| id != detail.id) {
        return Err(RundeckError::BadArg(format!(
            "the definition's uuid is not {}'s ({}); import changes that job in place",
            agent::path_of(&detail),
            detail.id
        )));
    }
    check_production(&detail, &definition, ask.production, prod_envs)?;
    check_whole(&definition, &live(session, &detail).await?)?;
    put(session, detail, definition).await
}

async fn put(
    session: &Session,
    detail: JobDetail,
    mut definition: Value,
) -> RundeckResult<Imported> {
    if let Value::Object(fields) = &mut definition {
        fields.insert("uuid".into(), Value::String(detail.id.clone()));
        fields.insert("id".into(), Value::String(detail.id.clone()));
    }
    let answer: Value = post_json_at_version(
        session,
        JOB_DEFINITION_API_VERSION,
        &format!("/project/{}/jobs/import", seg(&detail.project)?),
        &Value::Array(vec![definition]),
        &[
            ("fileformat", "json".into()),
            ("dupeOption", "update".into()),
            ("uuidOption", "preserve".into()),
        ],
    )
    .await?;
    if let Some(failed) = answer
        .get("failed")
        .and_then(Value::as_array)
        .and_then(|failed| failed.first())
    {
        let why = failed
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("Rundeck refused the definition");
        return Err(RundeckError::Api(why.to_owned()));
    }
    let job = answer
        .get("succeeded")
        .and_then(Value::as_array)
        .and_then(|done| done.first())
        .ok_or_else(|| RundeckError::Api("Rundeck skipped the definition".into()))?;
    let field = |key: &str| job.get(key).and_then(Value::as_str);
    let path = match (field("group"), field("name")) {
        (Some(group), Some(name)) if !group.is_empty() => format!("{group}/{name}"),
        (_, Some(name)) => name.to_owned(),
        _ => agent::path_of(&detail),
    };
    Ok(Imported {
        project: detail.project,
        path,
        id: field("id").unwrap_or(&detail.id).to_owned(),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditAsk {
    job: String,
    project: Option<String>,
    revision: String,
    changes: Vec<Change>,
    #[serde(default)]
    production: bool,
}

/// One change to a definition, made where a JSON pointer such as `/sequence/commands/2` says.
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
enum Change {
    Add {
        path: String,
        value: Value,
    },
    Replace {
        path: String,
        value: Value,
    },
    Remove {
        path: String,
    },
    ReplaceText {
        path: String,
        old: String,
        value: String,
    },
}

#[derive(Debug, Serialize)]
pub struct Edited {
    #[serde(flatten)]
    job: Imported,
    /// What to pass to the next edit; none when the job could not be read again.
    revision: Option<String>,
}

/// What tells a job from the others, which an edit leaves as it is.
const IDENTITY: [&str; 5] = ["id", "uuid", "name", "group", "project"];

pub async fn edit(session: &Session, ask: EditAsk, prod_envs: &[String]) -> RundeckResult<Edited> {
    if ask.changes.is_empty() {
        return Err(RundeckError::BadArg("changes names no change".into()));
    }
    let detail = resolve(session, &ask.job, ask.project.as_deref()).await?;
    let live_now = live(session, &detail).await?;
    if revision(&live_now) != ask.revision {
        return Err(RundeckError::BadArg(format!(
            "nothing was changed: {} is no longer as it was at that revision. Export it again and make the change to what it holds now.",
            agent::path_of(&detail)
        )));
    }
    let mut definition = live_now.clone();
    for change in &ask.changes {
        change
            .apply(&mut definition)
            .map_err(|why| RundeckError::BadArg(format!("nothing was changed: {why}")))?;
    }
    if let Some(key) = (IDENTITY.iter()).find(|key| definition.get(**key) != live_now.get(**key)) {
        return Err(RundeckError::BadArg(format!(
            "nothing was changed: edit leaves a job's {key} as it is"
        )));
    }
    check_production(&detail, &definition, ask.production, prod_envs)?;
    check_whole(&definition, &live_now)?;
    let job = put(session, detail.clone(), definition).await?;
    Ok(Edited {
        job,
        revision: (live(session, &detail).await.ok()).map(|definition| revision(&definition)),
    })
}

impl Change {
    fn apply(&self, definition: &mut Value) -> Result<(), String> {
        let (Change::Add { path, .. }
        | Change::Replace { path, .. }
        | Change::Remove { path }
        | Change::ReplaceText { path, .. }) = self;
        let nothing = || format!("the job has nothing at {path}");
        match self {
            Change::Replace { value, .. } => {
                holder(definition, path)?;
                *definition.pointer_mut(path).ok_or_else(nothing)? = value.clone();
            }
            Change::ReplaceText { old, value, .. } => {
                let Some(Value::String(text)) = definition.pointer_mut(path) else {
                    return Err(format!("the job has no text at {path}"));
                };
                let found = if old.is_empty() {
                    0
                } else {
                    text.matches(old.as_str()).count()
                };
                if found != 1 {
                    return Err(format!(
                        "old is in {path} {found} times, and it must be there once"
                    ));
                }
                *text = text.replacen(old.as_str(), value, 1);
            }
            Change::Add { value, .. } => match holder(definition, path)? {
                (Value::Array(items), key) => {
                    let at = match key.as_str() {
                        "-" => items.len(),
                        _ => position(&key)
                            .filter(|at| *at <= items.len())
                            .ok_or_else(|| {
                                format!("{path} is past the end of its list of {}", items.len())
                            })?,
                    };
                    items.insert(at, value.clone());
                }
                (Value::Object(fields), key) if !fields.contains_key(&key) => {
                    fields.insert(key, value.clone());
                }
                (Value::Object(_), _) => {
                    return Err(format!("the job already has {path}; replace it instead"));
                }
                _ => return Err(nothing()),
            },
            Change::Remove { .. } => match holder(definition, path)? {
                (Value::Array(items), key) => {
                    let at = position(&key)
                        .filter(|at| *at < items.len())
                        .ok_or_else(nothing)?;
                    items.remove(at);
                }
                (Value::Object(fields), key) => {
                    fields.remove(&key).ok_or_else(nothing)?;
                }
                _ => return Err(nothing()),
            },
        }
        Ok(())
    }
}

/// What holds the part `path` points at, and that part's name or position in it.
fn holder<'a>(definition: &'a mut Value, path: &str) -> Result<(&'a mut Value, String), String> {
    let (parent, key) = path.rsplit_once('/').ok_or_else(|| {
        format!("{path:?} is not a path to a part of the job, such as /sequence/commands/2")
    })?;
    let holder = (definition.pointer_mut(parent))
        .ok_or_else(|| format!("the job has nothing at {parent}"))?;
    Ok((holder, key.replace("~1", "/").replace("~0", "~")))
}

fn position(key: &str) -> Option<usize> {
    let plain =
        key.bytes().all(|byte| byte.is_ascii_digit()) && (key == "0" || !key.starts_with('0'));
    plain.then(|| key.parse().ok()).flatten()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use super::*;

    const JOB: &str = "0a1b2c3d-0000-4000-8000-000000000001";

    /// A stand-in Rundeck holding one made-up job with a script too long for one answer.
    struct StandIn {
        session: Session,
        job: Arc<Mutex<Value>>,
    }

    fn stand_in() -> StandIn {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let script: String = (0..1200).map(|line| format!("echo {line}\n")).collect();
        let job = Arc::new(Mutex::new(json!({
            "id": JOB, "uuid": JOB, "name": "greet", "group": "demo", "project": "toys",
            "sequence": { "keepgoing": false, "commands": [
                { "exec": "echo first" },
                { "script": script, "description": "long" },
                { "exec": "echo last" },
            ] },
        })));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let held = job.clone();
        std::thread::spawn(move || {
            listener
                .incoming()
                .flatten()
                .for_each(|stream| serve(stream, &held));
        });
        StandIn {
            session: Session {
                url,
                token: "made-up".into(),
                user: "tester".into(),
                allow_insecure_private_http: true,
            },
            job,
        }
    }

    fn serve(mut stream: TcpStream, job: &Mutex<Value>) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut first = String::new();
        reader.read_line(&mut first).unwrap();
        let mut length = 0;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                break;
            }
            if let Some(("content-length", value)) = (line.to_lowercase().trim()).split_once(": ") {
                length = value.parse().unwrap();
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let answer = if first.starts_with("POST ") {
            let sent: Value = serde_json::from_slice(&body).unwrap();
            *job.lock().unwrap() = sent[0].clone();
            json!({ "succeeded": [{ "id": JOB, "name": "greet", "group": "demo" }], "failed": [] })
        } else {
            json!([*job.lock().unwrap()])
        }
        .to_string();
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            answer.len()
        );
        stream
            .write_all(format!("{head}{answer}").as_bytes())
            .unwrap();
    }

    impl StandIn {
        fn holds(&self) -> Value {
            self.job.lock().unwrap().clone()
        }

        async fn export(&self) -> Exported {
            let ask = json!({ "job": JOB });
            export(&self.session, serde_json::from_value(ask).unwrap())
                .await
                .unwrap()
        }

        async fn import(&self, definition: Value) -> Result<(), String> {
            let ask = json!({ "job": JOB, "definition": definition });
            import(&self.session, serde_json::from_value(ask).unwrap(), &[])
                .await
                .map(drop)
                .map_err(|error| error.to_string())
        }

        async fn edit(&self, revision: &str, changes: Value) -> Result<Edited, String> {
            let ask = json!({ "job": JOB, "revision": revision, "changes": changes });
            edit(&self.session, serde_json::from_value(ask).unwrap(), &[])
                .await
                .map_err(|error| error.to_string())
        }
    }

    #[tokio::test]
    async fn a_definition_cut_when_it_was_read_never_replaces_the_job() {
        let rundeck = stand_in();
        let before = rundeck.holds();
        for mark in [cut::MARK, "…"] {
            let mut definition = rundeck.export().await.definition;
            let script = definition
                .pointer_mut("/sequence/commands/1/script")
                .unwrap();
            let start: String = script.as_str().unwrap().chars().take(3000).collect();
            *script = format!("{start}{mark}").into();
            let refused = rundeck.import(definition).await.unwrap_err();
            assert!(refused.contains("nothing was changed"), "{refused}");
        }
        let mut definition = rundeck.export().await.definition;
        definition["omitted"] = json!(["definition.sequence.commands[1].script"]);
        rundeck.import(definition).await.unwrap_err();
        assert_eq!(rundeck.holds(), before);

        let mut whole = rundeck.export().await.definition;
        whole["description"] = "changed".into();
        rundeck.import(whole.clone()).await.unwrap();
        assert_eq!(rundeck.holds(), whole);
    }

    #[tokio::test]
    async fn an_edit_changes_only_what_it_names() {
        let rundeck = stand_in();
        let mut expected = rundeck.holds();
        let read = rundeck.export().await.revision;
        let done = rundeck
            .edit(
                &read,
                json!([
                    { "op": "add", "path": "/sequence/commands/1", "value": { "exec": "echo new" } },
                    { "op": "replaceText", "path": "/sequence/commands/2/script", "old": "echo 700\n", "value": "echo seven hundred\n" },
                    { "op": "remove", "path": "/sequence/commands/3" },
                ]),
            )
            .await
            .unwrap();
        let commands = expected["sequence"]["commands"].as_array_mut().unwrap();
        let script = commands[1]["script"].as_str().unwrap();
        let script = script.replace("echo 700\n", "echo seven hundred\n");
        commands[1]["script"] = script.into();
        commands.insert(1, json!({ "exec": "echo new" }));
        commands.remove(3);
        assert_eq!(rundeck.holds(), expected);
        assert_eq!(done.revision, Some(rundeck.export().await.revision));
    }

    #[tokio::test]
    async fn an_edit_is_refused_when_the_job_changed_or_would_move() {
        let rundeck = stand_in();
        let read = rundeck.export().await.revision;
        rundeck.job.lock().unwrap()["description"] = "someone else's".into();
        let before = rundeck.holds();
        let last = json!([{ "op": "remove", "path": "/sequence/commands/2" }]);
        let refused = rundeck.edit(&read, last).await.unwrap_err();
        assert!(refused.contains("no longer as it was"), "{refused}");

        let read = rundeck.export().await.revision;
        for changes in [
            json!([{ "op": "replace", "path": "/group", "value": "production" }]),
            json!([{ "op": "replace", "path": "", "value": {} }]),
            json!([{ "op": "remove", "path": "/sequence/commands/3" }]),
            json!([{ "op": "replaceText", "path": "/sequence/commands/1/script", "old": "echo", "value": "" }]),
            json!([{ "op": "replace", "path": "/sequence/commands/0/exec", "value": format!("echo {}", cut::MARK) }]),
        ] {
            rundeck.edit(&read, changes).await.unwrap_err();
        }
        assert_eq!(rundeck.holds(), before);
    }
}
