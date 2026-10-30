//! A chat's changes as the core sends them, read whole on the thread that hears the core, so the
//! window applies typed changes and never reads their JSON again.

use serde::de::DeserializeOwned;
use serde_json::Value;
use sikemux_wire::chat::{ChatChange, ChatHead, ChatMessage, ChatPart, ChatView};

/// Some fields of a `T`, each read already, put in place in turn.
pub type Fields<T> = Vec<Field<T>>;

pub enum Changed {
    Head {
        set: Fields<ChatHead>,
        /// The number a delivery that catches the window up follows.
        follows: Option<u64>,
    },
    Row {
        row: ChatMessage,
        within: Option<String>,
    },
    Part {
        row: String,
        part: ChatPart,
        within: Option<String>,
    },
    Text {
        row: String,
        part: String,
        text: String,
        md: Option<Value>,
        within: Option<String>,
    },
    Set {
        row: String,
        set: Fields<ChatMessage>,
        /// Its parts or the letters it holds moved, which its markdown is read from.
        docs: bool,
        within: Option<String>,
    },
    Keep {
        rows: Vec<String>,
    },
    Drop {
        from: String,
    },
    View {
        view: Box<ChatView>,
    },
}

impl Changed {
    pub fn follows(&self) -> Option<u64> {
        match self {
            Changed::Head { follows, .. } => *follows,
            _ => None,
        }
    }
}

/// Reads the list of `ChatChange`s the core sent.
pub fn read(changes: Value) -> Result<Vec<Changed>, String> {
    let changes: Vec<ChatChange> =
        serde_json::from_value(changes).map_err(|error| error.to_string())?;
    changes
        .into_iter()
        .map(|change| {
            Ok(match change {
                ChatChange::Head { set, follows } => Changed::Head {
                    set: fields(set, head_field)?,
                    follows,
                },
                ChatChange::Row { row, within } => Changed::Row { row, within },
                ChatChange::Part { row, part, within } => Changed::Part { row, part, within },
                ChatChange::Text {
                    row,
                    part,
                    text,
                    md,
                    within,
                } => Changed::Text {
                    row,
                    part,
                    text,
                    md,
                    within,
                },
                ChatChange::Set { row, set, within } => Changed::Set {
                    row,
                    docs: set.get("parts").is_some() || set.get("letters").is_some(),
                    set: fields(set, row_field)?,
                    within,
                },
                ChatChange::Keep { rows } => Changed::Keep { rows },
                ChatChange::Drop { from } => Changed::Drop { from },
                ChatChange::View { view } => Changed::View { view },
            })
        })
        .collect()
}

pub type Field<T> = Box<dyn FnOnce(&mut T) + Send>;
/// Reads one field of a `T` from its name on the wire, or none for a field this app does not know.
type ReadField<T> = fn(&str, Value) -> Result<Option<Field<T>>, serde_json::Error>;

/// Reads each field `set` names by `field`, leaving out those this app does not know.
fn fields<T>(set: Value, field: ReadField<T>) -> Result<Fields<T>, String> {
    let Value::Object(set) = set else {
        return Err("a set of fields is not an object".into());
    };
    let mut read = Vec::with_capacity(set.len());
    for (key, value) in set {
        if let Some(field) = field(&key, value).map_err(|error| format!("{key}: {error}"))? {
            read.push(field);
        }
    }
    Ok(read)
}

/// A field's new value: none gives it its default, as a field left out reads.
fn value<T: DeserializeOwned + Default, S>(
    value: Value,
    _: fn(&S) -> &T,
) -> Result<T, serde_json::Error> {
    match value {
        Value::Null => Ok(T::default()),
        value => serde_json::from_value(value),
    }
}

/// A field's new value, for a field that has no default to fall back on.
fn required<T: DeserializeOwned, S>(value: Value, _: fn(&S) -> &T) -> Result<T, serde_json::Error> {
    serde_json::from_value(value)
}

/// The fields of a struct by their names on the wire, each read into the field's own type. Every
/// field is named, so the build fails when the struct gains one this does not.
macro_rules! named_fields {
    (@value $read:ident, $ty:ident, $field:ident) => {
        value($read, |on: &$ty| &on.$field)
    };
    (@value $read:ident, $ty:ident, $field:ident required) => {
        required($read, |on: &$ty| &on.$field)
    };
    ($read:ident, $ty:ident { $($key:literal => $field:ident $($required:ident)?),* $(,)? }) => {
        fn $read(key: &str, read: Value) -> Result<Option<Field<$ty>>, serde_json::Error> {
            #[allow(unused)]
            fn every_field_named(on: &$ty) {
                let $ty { $($field: _),* } = on;
            }
            Ok(Some(match key {
                $($key => {
                    let read = named_fields!(@value read, $ty, $field $($required)?)?;
                    Box::new(move |on: &mut $ty| on.$field = read)
                })*
                _ => return Ok(None),
            }))
        }
    };
}

named_fields!(head_field, ChatHead {
    "sessionId" => session_id,
    "connection" => connection,
    "running" => running,
    "turned" => turned,
    "permissionMode" => permission_mode,
    "changingMode" => changing_mode,
    "turnOpening" => turn_opening,
    "retrying" => retrying,
    "error" => error,
    "failure" => failure,
    "stopReason" => stop_reason,
    "title" => title,
    "usage" => usage,
    "plan" => plan,
    "commands" => commands,
    "capabilities" => capabilities,
    "setup" => setup,
    "model" => model,
    "effort" => effort,
    "permissions" => permissions,
    "tasks" => tasks,
    "runningSubagents" => running_subagents,
    "queued" => queued,
    "triggers" => triggers,
    "earlierLost" => earlier_lost,
    "ended" => ended,
    "activeTool" => active_tool,
    "draft" => draft,
});

named_fields!(row_field, ChatMessage {
    "id" => id,
    "role" => role required,
    "promptId" => prompt_id,
    "parts" => parts,
    "attachments" => attachments,
    "context" => context,
    "letters" => letters,
    "sentAt" => sent_at,
    "endedAt" => ended_at,
    "opensTurn" => opens_turn,
    "meta" => meta,
    "fold" => fold,
    "reactions" => reactions,
    "fromEvent" => from_event,
});
