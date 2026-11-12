//! Tools whose names start with a family's, such as browser_click and browser_type,
//! offered to agents as one tool, `browser`, that takes the rest of the name as its
//! `action`. Each keeps its own schema, method and name inside Sikemux, so a call is
//! checked against the action it names, and only the list agents read is merged.

use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::manifest::{spelled, Tool};

pub const ACTION: &str = "action";

#[derive(Clone, Deserialize, PartialEq)]
pub struct Family {
    pub name: String,
    description: String,
}

impl Family {
    /// Whether `name` is this family's or one of its members'.
    pub fn owns(&self, name: &str) -> bool {
        name == self.name || self.action(name).is_some()
    }

    fn action<'a>(&self, tool: &'a str) -> Option<&'a str> {
        tool.strip_prefix(self.name.as_str())?.strip_prefix('_')
    }

    fn members<'a>(&self, tools: &[&'a Tool]) -> Vec<(&'a str, &'a Tool)> {
        tools
            .iter()
            .filter_map(|tool| Some((self.action(&tool.name)?, *tool)))
            .collect()
    }
}

/// What `tools/list` offers: each family as one tool, where its first member stood,
/// and every other tool as it is.
pub fn declarations(families: &[Family], tools: &[&Tool]) -> Vec<Value> {
    let mut listed: Vec<Value> = Vec::new();
    let mut merged: Vec<&str> = Vec::new();
    for tool in tools {
        match families
            .iter()
            .find(|family| family.action(&tool.name).is_some())
        {
            Some(family) if merged.contains(&family.name.as_str()) => {}
            Some(family) => {
                merged.push(&family.name);
                listed.push(declaration(family, &family.members(tools)));
            }
            None => listed.push(tool.declaration()),
        }
    }
    listed
}

fn declaration(family: &Family, members: &[(&str, &Tool)]) -> Value {
    let declared: Vec<(&str, Value)> = members
        .iter()
        .map(|(action, tool)| (*action, tool.declaration()))
        .collect();
    let mut description = family.description.clone();
    for (action, tool) in &declared {
        description.push_str(&format!("\n{action}: {}", text(tool, "description")));
    }
    let mut properties = Map::new();
    properties.insert(
        ACTION.into(),
        json!({ "enum": members.iter().map(|(action, _)| *action).collect::<Vec<_>>() }),
    );
    let mut names: Vec<&str> = Vec::new();
    for (_, tool) in &declared {
        for name in schema(tool, "properties")
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, _)| name)
        {
            if !names.contains(&name.as_str()) {
                names.push(name);
            }
        }
    }
    for name in names {
        let given: Vec<(&str, &Value)> = declared
            .iter()
            .filter_map(|(action, tool)| Some((*action, schema(tool, "properties").get(name)?)))
            .collect();
        properties.insert(
            name.to_owned(),
            combined(&given, given.len() == declared.len()),
        );
    }
    let mut required = vec![ACTION.to_owned()];
    if let Some(((_, first), rest)) = declared.split_first() {
        for name in schema(first, "required").as_array().into_iter().flatten() {
            if rest.iter().all(|(_, tool)| {
                schema(tool, "required")
                    .as_array()
                    .is_some_and(|all| all.contains(name))
            }) {
                required.extend(name.as_str().map(str::to_owned));
            }
        }
    }
    json!({
        "name": family.name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
    })
}

fn text<'a>(declared: &'a Value, key: &str) -> &'a str {
    declared[key].as_str().unwrap_or_default()
}

fn schema<'a>(declared: &'a Value, key: &str) -> &'a Value {
    &declared["inputSchema"][key]
}

/// One property as the actions that take it describe it: the same schema when they agree,
/// else the first's with every listed value any of them takes. Unless `everyone` takes it,
/// its description starts with the actions that do, so an agent does not hand it to others.
fn combined(given: &[(&str, &Value)], everyone: bool) -> Value {
    let Some(((_, first), _)) = given.split_first() else {
        return json!({});
    };
    if everyone && given.iter().all(|(_, schema)| schema == first) {
        return (*first).clone();
    }
    let mut merged = (*first).clone();
    let Some(fields) = merged.as_object_mut() else {
        return merged;
    };
    fields.remove("description");
    let mut choices: Vec<Value> = Vec::new();
    for choice in given
        .iter()
        .flat_map(|(_, schema)| schema["enum"].as_array().into_iter().flatten())
    {
        if !choices.contains(choice) {
            choices.push(choice.clone());
        }
    }
    if !choices.is_empty() {
        fields.insert("enum".into(), Value::Array(choices));
    }
    let mut said: Vec<(Option<&str>, Vec<&str>)> = Vec::new();
    for (action, schema) in given {
        let description = schema["description"].as_str();
        if everyone && description.is_none() {
            continue;
        }
        match said.iter_mut().find(|(text, _)| *text == description) {
            Some((_, actions)) => actions.push(action),
            None => said.push((description, vec![action])),
        }
    }
    let parts: Vec<String> = match said.as_slice() {
        [(Some(description), _)] if everyone => vec![(*description).to_owned()],
        many => many
            .iter()
            .map(|(description, actions)| match description {
                Some(description) => format!("{}: {description}", actions.join(", ")),
                None => actions.join(", "),
            })
            .collect(),
    };
    if !parts.is_empty() {
        fields.insert("description".into(), json!(parts.join("; ")));
    }
    merged
}

/// A call of a family's tool, as the member its action names takes it.
pub struct Chosen<'a> {
    pub tool: &'a Tool,
    pub arguments: Value,
}

/// The member a call of `name` means, with its arguments settled and checked, when
/// `name` is a family; an error names the action, so the agent knows whose field it lacks.
pub fn choose<'a>(
    families: &[Family],
    tools: &[&'a Tool],
    name: &str,
    arguments: &Value,
) -> Option<Result<Chosen<'a>, String>> {
    let family = families.iter().find(|family| family.name == name)?;
    Some(chosen(family, &family.members(tools), arguments))
}

/// The member a call names, without checking its arguments, so a call can be put in
/// line with others on its tab before it runs.
pub fn member<'a>(
    families: &[Family],
    tools: &[&'a Tool],
    name: &str,
    arguments: &Value,
) -> Option<&'a Tool> {
    let family = families.iter().find(|family| family.name == name)?;
    let asked = arguments.get(ACTION)?.as_str()?;
    named(&family.name, &family.members(tools), asked).map(|(_, tool)| tool)
}

fn named<'a>(
    family: &str,
    members: &[(&'a str, &'a Tool)],
    asked: &str,
) -> Option<(&'a str, &'a Tool)> {
    let asked = asked
        .strip_prefix(family)
        .and_then(|rest| rest.strip_prefix('_'))
        .unwrap_or(asked);
    members
        .iter()
        .find(|(action, _)| *action == asked)
        .or_else(|| {
            members
                .iter()
                .find(|(action, _)| spelled(action) == spelled(asked))
        })
        .copied()
}

fn chosen<'a>(
    family: &Family,
    members: &[(&'a str, &'a Tool)],
    arguments: &Value,
) -> Result<Chosen<'a>, String> {
    let actions: Vec<String> = members
        .iter()
        .map(|(action, _)| format!("'{action}'"))
        .collect();
    let Value::Object(object) = arguments else {
        return Err(format!("{arguments} is not of type 'object'"));
    };
    let mut object = object.clone();
    let asked = match object.remove(ACTION) {
        Some(Value::String(asked)) => asked,
        Some(other) => {
            return Err(format!(
                "'{ACTION}' is {other}, not one of [{}]",
                actions.join(", ")
            ))
        }
        None => {
            return Err(format!(
                "'{ACTION}' is a required property: one of [{}]",
                actions.join(", ")
            ))
        }
    };
    let Some((action, tool)) = named(&family.name, members, &asked) else {
        return Err(format!("'{asked}' is not one of [{}]", actions.join(", ")));
    };
    let arguments = tool.settle(Value::Object(object));
    let failed = |message: String| format!("{} {action}: {message}", family.name);
    if let Some(misplaced) = elsewhere(members, action, tool, &arguments) {
        return Err(failed(misplaced));
    }
    tool.validate(&arguments).map_err(failed)?;
    Ok(Chosen { tool, arguments })
}

/// The fields of other actions a call gave this one, with the actions that take them.
fn elsewhere(
    members: &[(&str, &Tool)],
    action: &str,
    tool: &Tool,
    arguments: &Value,
) -> Option<String> {
    let notes: Vec<String> = arguments
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, _)| name)
        .filter(|name| !tool.takes(name))
        .filter_map(|name| {
            let takers: Vec<&str> = members
                .iter()
                .filter(|(_, other)| other.takes(name))
                .map(|(action, _)| *action)
                .collect();
            (!takers.is_empty())
                .then(|| format!("'{name}' goes with {}, not {action}", takers.join(", ")))
        })
        .collect();
    (!notes.is_empty()).then(|| notes.join("; "))
}
