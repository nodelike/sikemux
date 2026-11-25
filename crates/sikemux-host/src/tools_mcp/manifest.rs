//! The tool surface: tools/tools.json and the guide, as the running core
//! serves them, or as this binary was built when the core cannot be asked.

use std::collections::HashSet;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::{json, Map, Value};
use sikemux_core::cli::manifest::{
    format_tool_line, gateway_instructions_with_list, is_tool_offered, GATEWAY_CALL, GUIDE, TOOLS,
};
use sikemux_core::cli::protocol::RESOURCES_OFFER;

use super::family::{self, Chosen, Family};

/// A tool from tools/tools.json, or one a plugin offers, which the app
/// describes in the same shape.
#[derive(Deserialize)]
pub struct Tool {
    pub name: String,
    pub method: String,
    description: String,
    properties: Map<String, Value>,
    required: Vec<String>,
    /// Set on tools listed only where they can work, such as `simulator` for
    /// the device tools on a Mac with Xcode or a computer with the Android SDK.
    #[serde(default)]
    pub offered: Option<String>,
    /// How the agent is shown the app's answer when it is not plain JSON:
    /// `screenshot` or `image`.
    #[serde(default)]
    pub answer: Option<String>,
    /// How the app shows a call of it, which says whether it changes anything.
    #[serde(default)]
    row: Row,
    /// The tool a plugin offers its tools as, which the core says beside each.
    #[serde(default)]
    family: Option<Family>,
    /// Set on the few tools nearly every chat calls, so Claude Code lists them up front
    /// instead of spending a round trip to look each one up.
    #[serde(default, rename = "alwaysLoad")]
    always_load: bool,
}

#[derive(Default, Deserialize)]
struct Row {
    kind: Option<String>,
    verb: Option<String>,
}

#[derive(Deserialize)]
struct Guide {
    name: String,
    description: String,
}

pub struct Manifest {
    guide: Guide,
    guide_text: String,
    tools: Vec<Tool>,
    families: Vec<Family>,
}

impl Manifest {
    /// The copy this binary was built with, which must read in full.
    pub fn load() -> Self {
        #[derive(Deserialize)]
        struct Built {
            guide: Guide,
            tools: Vec<Tool>,
            #[serde(default)]
            families: Vec<Family>,
        }
        #[expect(
            clippy::expect_used,
            reason = "tools.json is built into the binary and checked before every release"
        )]
        let built: Built =
            serde_json::from_str(TOOLS).expect("tools/tools.json is a valid tool manifest");
        Self {
            guide: built.guide,
            guide_text: GUIDE.to_owned(),
            tools: built.tools,
            families: built.families,
        }
    }

    /// Whether a call of `method` only reads, searches or waits, so sending it a second time
    /// does no more than the first.
    pub fn repeatable(method: &str) -> bool {
        static METHODS: OnceLock<HashSet<String>> = OnceLock::new();
        METHODS
            .get_or_init(|| {
                (Self::load().tools.into_iter())
                    .filter(|tool| {
                        let Row { kind, verb } = &tool.row;
                        match (kind.as_deref(), verb.as_deref()) {
                            (_, Some("scroll" | "show")) => false,
                            (_, Some("wait")) | (Some("read" | "search"), _) => true,
                            _ => false,
                        }
                    })
                    .map(|tool| tool.method)
                    .collect()
            })
            .contains(method)
    }

    /// What the core answers `tools.manifest` with. A tool this binary cannot
    /// read is left out rather than losing the rest.
    pub fn served(answer: &Value) -> Option<Self> {
        let manifest = answer.get("manifest")?;
        Some(Self {
            guide: Guide::deserialize(manifest.get("guide")?).ok()?,
            guide_text: answer.get("guide")?.as_str()?.to_owned(),
            tools: manifest
                .get("tools")?
                .as_array()?
                .iter()
                .filter_map(|tool| Tool::deserialize(tool).ok())
                .collect(),
            families: manifest
                .get("families")
                .and_then(|families| Vec::<Family>::deserialize(families).ok())
                .unwrap_or_default(),
        })
    }

    /// Leaves out the simulator tools unless the app said this Mac can run
    /// them, and tools offered on a condition this binary does not know.
    pub fn offering(mut self, simulator: bool) -> Self {
        self.tools
            .retain(|tool| is_tool_offered(tool.offered.as_deref(), simulator, false, true));
        self
    }

    /// Offers the tools for things only one agent uses at a time, such as `hold`, as any other
    /// when `resources`, and leaves them out otherwise; call it before [`Self::offering`].
    pub fn with_resources(mut self, resources: bool) -> Self {
        self.tools.retain_mut(|tool| match tool.offered.as_deref() {
            Some(RESOURCES_OFFER) if resources => {
                tool.offered = None;
                true
            }
            Some(RESOURCES_OFFER) => false,
            _ => true,
        });
        self
    }

    fn offers_simulator(&self) -> bool {
        self.tools
            .iter()
            .any(|tool| tool.offered.as_deref() == Some("simulator"))
    }

    pub fn guide_name(&self) -> &str {
        &self.guide.name
    }

    /// The guide's opening, which every agent reads, or one topic of it.
    pub fn guide_text(&self, topic: Option<&str>) -> Result<&str, String> {
        let guide = self.guide_text.as_str();
        let sections = guide_topics(guide);
        match topic {
            None => Ok(without_front_matter(
                sections
                    .first()
                    .map_or(guide, |(_, start)| guide.get(..*start).unwrap_or(guide)),
            )),
            Some(wanted) => sections
                .iter()
                .enumerate()
                .find(|(_, (name, _))| *name == wanted)
                .or_else(|| {
                    [true, false].into_iter().find_map(|whole| {
                        let close: Vec<_> = sections
                            .iter()
                            .enumerate()
                            .filter(|(_, (name, _))| near(name, wanted, whole))
                            .collect();
                        match close.as_slice() {
                            [one] => Some(*one),
                            _ => None,
                        }
                    })
                })
                .map(|(index, (_, start))| {
                    let end = sections
                        .get(index + 1)
                        .map_or(guide.len(), |(_, next)| *next);
                    guide.get(*start..end).unwrap_or_default().trim_end()
                })
                .ok_or_else(|| {
                    let names: Vec<&str> = sections.iter().map(|(name, _)| *name).collect();
                    format!("No guide topic '{wanted}'. Topics: {}", names.join(", "))
                }),
        }
    }

    /// The parts of the guide that read differently from `before`: `None`
    /// for the opening, else a topic by name.
    pub fn guide_changes(&self, before: &str) -> Vec<Option<String>> {
        let earlier = guide_parts(before);
        guide_parts(&self.guide_text)
            .into_iter()
            .filter(|(name, text)| earlier.get(name) != Some(text))
            .map(|(name, _)| name.map(str::to_owned))
            .collect()
    }

    pub fn guide_source(&self) -> &str {
        &self.guide_text
    }

    pub fn instructions(&self) -> String {
        sikemux_core::cli::manifest::instructions_for(&self.guide.name, self.offers_simulator())
    }

    pub fn gateway_instructions(&self, plugins: &[Tool], setup: bool) -> String {
        gateway_instructions_with_list(
            &self.guide.name,
            self.offers_simulator(),
            &self.offered_summary(plugins, setup),
        )
    }

    pub fn gateway_declarations(&self) -> Vec<Value> {
        let mut guide_props = guide_properties(&self.guide_text);
        guide_props.insert(
            "tool".into(),
            json!({
                "type": "string",
                "description": "Tool name to inspect its parameters and description."
            }),
        );
        let mut call_props = Map::new();
        call_props.insert(
            "name".into(),
            json!({ "type": "string", "description": "Sikemux tool name." }),
        );
        call_props.insert(
            "arguments".into(),
            json!({ "type": "object", "description": "Arguments for that tool." }),
        );
        vec![
            declaration(
                &self.guide.name,
                "Read Sikemux workflow topics, list available tools, or inspect one tool's parameters before calling it.",
                &guide_props,
                &[],
            ),
            declaration(
                GATEWAY_CALL,
                &format!(
                    "Invoke a Sikemux tool by name. Call {} with tool: \"<name>\" first if unsure of its parameters.",
                    self.guide.name
                ),
                &call_props,
                &["name".to_owned()],
            ),
        ]
    }

    pub fn offered_summary(&self, plugins: &[Tool], setup: bool) -> String {
        self.declarations(plugins, setup)
            .into_iter()
            .filter_map(|decl| {
                let name = decl.get("name")?.as_str()?;
                if name == self.guide.name {
                    return None;
                }
                let description = decl
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let actions: Vec<&str> = decl
                    .pointer("/inputSchema/properties/action/enum")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                Some(format_tool_line(name, &actions, description))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn is_offered(&self, plugins: &[Tool], setup: bool, name: &str) -> bool {
        name != self.guide.name
            && self
                .declarations(plugins, setup)
                .iter()
                .any(|decl| decl.get("name").and_then(Value::as_str) == Some(name))
    }

    pub fn family_hint(
        &self,
        plugins: &[Tool],
        setup: bool,
        name: &str,
    ) -> Option<(String, String)> {
        self.declarations(plugins, setup)
            .into_iter()
            .find_map(|decl| {
                let family = decl.get("name")?.as_str()?;
                let action = name.strip_prefix(family)?.strip_prefix('_')?;
                let actions = decl
                    .pointer("/inputSchema/properties/action/enum")
                    .and_then(Value::as_array)?;
                actions
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|listed| listed == action)
                    .then(|| (family.to_owned(), action.to_owned()))
            })
    }

    pub fn tool_schema_text(
        &self,
        plugins: &[Tool],
        setup: bool,
        name: &str,
    ) -> Result<String, String> {
        let declared = self.declarations(plugins, setup);
        let found = declared.iter().find(|decl| {
            decl.get("name").and_then(Value::as_str) == Some(name) && name != self.guide.name
        });
        let Some(decl) = found else {
            if let Some((family, action)) = self.family_hint(plugins, setup, name) {
                return Err(format!(
                    "'{name}' is action '{action}' of '{family}'. Call {} with tool: \"{family}\", then call '{family}' with action: \"{action}\" in arguments.",
                    self.guide.name
                ));
            }
            return Err(format!(
                "Tool '{name}' is not available here. Call {} with no arguments to list available tools.",
                self.guide.name
            ));
        };
        Ok(format_tool_schema(decl))
    }

    pub fn tool(&self, name: &str) -> Option<&Tool> {
        self.tools.iter().find(|tool| tool.name == name)
    }

    /// Whether a name is already taken here, so a plugin tool can neither shadow it
    /// nor join one of these families.
    pub fn declares(&self, name: &str) -> bool {
        name == self.guide.name
            || self.tool(name).is_some()
            || self.families.iter().any(|family| family.owns(name))
    }

    /// The built-in tools, then those of `plugins` that take no name of theirs.
    fn with<'a>(&'a self, plugins: &'a [Tool]) -> Vec<&'a Tool> {
        self.tools
            .iter()
            .chain(plugins.iter().filter(|tool| !self.declares(&tool.name)))
            .collect()
    }

    fn families_with(&self, plugins: &[Tool]) -> Vec<Family> {
        let mut families = self.families.clone();
        for family in plugins.iter().filter_map(|tool| tool.family.as_ref()) {
            if !families.contains(family) && !self.declares(&family.name) {
                families.push(family.clone());
            }
        }
        families
    }

    /// The tool a call of `name` runs, without checking its arguments: a family's member
    /// by the action it names, or the built-in tool of that name.
    pub fn member(&self, name: &str, arguments: &Value) -> Option<&Tool> {
        family::member(&self.families, &self.with(&[]), name, arguments).or_else(|| self.tool(name))
    }

    /// The tool a call of a family's name means, with its arguments as that tool takes them.
    pub fn chosen<'a>(
        &'a self,
        plugins: &'a [Tool],
        name: &str,
        arguments: &Value,
    ) -> Option<Result<Chosen<'a>, String>> {
        family::choose(
            &self.families_with(plugins),
            &self.with(plugins),
            name,
            arguments,
        )
    }

    /// What `tools/list` gives: each family as one tool and the rest as they are, the
    /// built-in ones and the guide before the plugins'.
    pub fn declarations(&self, plugins: &[Tool], _setup: bool) -> Vec<Value> {
        let tools = self.with(plugins);
        let (built_in, theirs) = tools.split_at(self.tools.len());
        let families = self.families_with(plugins);
        let mut declared = family::declarations(&families, built_in);
        declared.push(declaration(
            &self.guide.name,
            &self.guide.description,
            &guide_properties(&self.guide_text),
            &[],
        ));
        declared.extend(family::declarations(&families, theirs));
        declared
    }
}

/// Whether a topic asked for by a near name, such as `page` for `pages`, or
/// `input` for `browser-input` when not `whole`, means `name`.
fn near(name: &str, wanted: &str, whole: bool) -> bool {
    let wanted = wanted.trim().to_lowercase();
    let same = |part: &str| {
        !wanted.is_empty() && part.trim_end_matches('s') == wanted.trim_end_matches('s')
    };
    match whole {
        true => same(name),
        false => name.split('-').any(same),
    }
}

/// Leaves out the name and description atop the guide file, which only matter to tools that read it as a skill.
fn without_front_matter(opening: &str) -> &str {
    opening
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map_or(opening, |(_, body)| body.trim_start())
}

/// Each `## name: Title` heading of the guide, with where its section starts.
/// Headings without a name belong to the opening every agent reads.
fn guide_topics(guide: &str) -> Vec<(&str, usize)> {
    let mut start = 0;
    let mut topics = Vec::new();
    for line in guide.split_inclusive('\n') {
        if let Some((name, _)) = line
            .strip_prefix("## ")
            .and_then(|heading| heading.split_once(": "))
        {
            topics.push((name, start));
        }
        start += line.len();
    }
    topics
}

/// The guide's opening, under `None`, and each topic by name, with its text.
fn guide_parts(guide: &str) -> std::collections::BTreeMap<Option<&str>, &str> {
    let topics = guide_topics(guide);
    let mut parts = std::collections::BTreeMap::new();
    parts.insert(
        None,
        topics
            .first()
            .map_or(guide, |(_, start)| guide.get(..*start).unwrap_or(guide)),
    );
    for (index, (name, start)) in topics.iter().enumerate() {
        let end = topics.get(index + 1).map_or(guide.len(), |(_, next)| *next);
        parts.insert(
            Some(*name),
            guide.get(*start..end).unwrap_or_default().trim_end(),
        );
    }
    parts
}

fn guide_properties(guide: &str) -> Map<String, Value> {
    let topics: Vec<&str> = guide_topics(guide)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let mut properties = Map::new();
    properties.insert("topic".into(), json!({ "enum": topics }));
    properties
}

/// Length limits are still checked here, but an agent never needs to read
/// them, so they are left out of what every request carries.
fn advertised(schema: &Value) -> Value {
    match schema {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .filter(|(key, _)| !matches!(key.as_str(), "maxLength" | "minLength"))
                .map(|(key, value)| (key.clone(), advertised(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(advertised).collect()),
        other => other.clone(),
    }
}

fn declaration(
    name: &str,
    description: &str,
    properties: &Map<String, Value>,
    required: &[String],
) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": advertised(&Value::Object(properties.clone())),
            "required": required,
            "additionalProperties": false,
        },
    })
}

impl Tool {
    pub fn takes(&self, argument: &str) -> bool {
        self.properties.contains_key(argument)
    }

    pub fn declaration(&self) -> Value {
        let mut declared = declaration(
            &self.name,
            &self.description,
            &self.properties,
            &self.required,
        );
        if let Some(fields) = declared.as_object_mut().filter(|_| self.always_load) {
            fields.insert("_meta".into(), json!({ "anthropic/alwaysLoad": true }));
        }
        declared
    }

    /// The wording matches what the agent used to read from the Python server,
    /// so a model that learned to recover from one of these still can.
    pub fn validate(&self, arguments: &Value) -> Result<(), String> {
        let Some(object) = arguments.as_object() else {
            return Err(format!("{} is not of type 'object'", describe(arguments)));
        };
        for (name, schema) in &self.properties {
            if let Some(value) = object.get(name) {
                check(value, schema, name, false)?;
            }
        }
        let unexpected: Vec<&String> = object
            .keys()
            .filter(|name| !self.properties.contains_key(*name))
            .collect();
        let hints: Vec<String> = unexpected
            .iter()
            .filter_map(|name| {
                if let Some(field) = object
                    .get(*name)
                    .and_then(Value::as_str)
                    .filter(|value| self.properties.contains_key(*value))
                {
                    return Some(format!("'{name}': '{field}' should be '{field}': <value>"));
                }
                self.closest(name)
                    .map(|known| format!("'{name}' should be '{known}'"))
            })
            .collect();
        let hint = if hints.is_empty() {
            String::new()
        } else {
            format!("; {}", hints.join(", "))
        };
        for name in &self.required {
            if !object.contains_key(name) {
                return Err(format!("'{name}' is a required property{hint}"));
            }
        }
        if unexpected.is_empty() {
            return Ok(());
        }
        let verb = if unexpected.len() == 1 { "was" } else { "were" };
        let listed: Vec<String> = unexpected.iter().map(|name| format!("'{name}'")).collect();
        Err(format!(
            "Additional properties are not allowed ({} {verb} unexpected){hint}",
            listed.join(", ")
        ))
    }

    /// The arguments as this tool takes them, so a call an agent got nearly right runs rather
    /// than coming back refused: a name it commonly uses for one of this tool's is renamed,
    /// `repo: "owner/name"` is split where the tool takes `owner` and `name`, a listed value
    /// written in another case or spelling is matched, a number past a bound is held at it, and one
    /// name given where a list is taken, or a list where one name is, is fitted to the field.
    pub fn settle(&self, arguments: Value) -> Value {
        let Value::Object(mut object) = arguments else {
            return arguments;
        };
        let unknown: Vec<String> = object
            .keys()
            .filter(|name| !self.properties.contains_key(*name))
            .cloned()
            .collect();
        for name in unknown {
            let Some(value) = object.get(&name) else {
                continue;
            };
            if let Some((owner, repo)) = self.split_repo(&name, value) {
                object.remove(&name);
                if let Some(owner) = owner {
                    object.entry("owner").or_insert(owner.into());
                }
                object.entry("name").or_insert(repo.into());
                continue;
            }
            let Some(meant) = self.alias(&name) else {
                continue;
            };
            if object.contains_key(meant) {
                continue;
            }
            let Some(mut value) = object.remove(&name) else {
                continue;
            };
            if meant == "timeoutMs" && in_seconds(&name, &value) {
                value = value
                    .as_f64()
                    .map_or(value, |seconds| json!((seconds * 1000.0) as u64));
            }
            object.insert(meant.to_owned(), value);
        }
        if let Some(Value::Array(names)) = object.get("to") {
            if self.properties.contains_key("to") && names.iter().all(Value::is_string) {
                let names: Vec<&str> = names.iter().filter_map(Value::as_str).collect();
                let joined = names.join(", ");
                object.insert("to".into(), joined.into());
            }
        }
        for (name, value) in object.iter_mut() {
            if let Some(schema) = self.properties.get(name) {
                settle_value(value, schema);
            }
        }
        Value::Object(object)
    }

    fn split_repo(&self, name: &str, value: &Value) -> Option<(Option<String>, String)> {
        let takes = |field: &str| self.properties.contains_key(field);
        if !matches!(name, "repo" | "repository" | "slug") || !takes("owner") || !takes("name") {
            return None;
        }
        let slug = value.as_str()?.trim_matches('/');
        Some(match slug.rsplit_once('/') {
            Some((owner, repo)) => (Some(owner.to_owned()), repo.to_owned()),
            None => (None, slug.to_owned()),
        })
    }

    /// A name agents use for one of this tool's properties, from habits other tools taught them.
    fn alias(&self, name: &str) -> Option<&'static str> {
        const SAME_MEANING: &[(&str, &[&str])] = &[
            (
                "script",
                &["expression", "code", "js", "javascript", "function"],
            ),
            ("selector", &["query", "css"]),
            ("expression", &["query", "filter", "where"]),
            ("tail", &["lines"]),
            ("processId", &["name", "process", "task", "taskid", "id"]),
            ("executionId", &["id", "execution", "execution_id"]),
            (
                "timeoutMs",
                &[
                    "timeout",
                    "timeout_ms",
                    "timeoutms",
                    "timeoutseconds",
                    "timeout_s",
                ],
            ),
            ("deltaY", &["y", "dy", "amount", "pixels"]),
            ("url", &["href", "link"]),
            (
                "to",
                &["agent", "agents", "agentid", "agent_id", "recipient"],
            ),
            ("agent", &["to", "agents", "agentid", "agent_id"]),
            (
                "agents",
                &["to", "agent", "agentid", "agent_id", "agentids"],
            ),
            ("text", &["value", "content", "message", "body"]),
            ("prompt", &["message", "text", "instructions"]),
            ("paths", &["files", "file", "path"]),
            ("index", &["element", "ref", "number"]),
            ("trigger", &["id", "triggerid", "trigger_id"]),
            ("target", &["run", "commit", "test", "path"]),
            ("process", &["processid", "executionid", "execution_id"]),
        ];
        let lowered = name.to_ascii_lowercase();
        SAME_MEANING
            .iter()
            .find(|(meant, others)| {
                self.properties.contains_key(*meant) && others.contains(&lowered.as_str())
            })
            .map(|(meant, _)| *meant)
    }

    /// The property an agent most likely meant by a name this tool lacks.
    fn closest(&self, name: &str) -> Option<&str> {
        let lowered = name.to_ascii_lowercase();
        self.alias(name).or_else(|| {
            self.properties
                .keys()
                .map(|candidate| {
                    (
                        candidate,
                        distance(&lowered, &candidate.to_ascii_lowercase()),
                    )
                })
                .filter(|(candidate, apart)| *apart <= 2 && *apart < candidate.len())
                .min_by_key(|(_, apart)| *apart)
                .map(|(candidate, _)| candidate.as_str())
        })
    }
}

/// Whether a timeout given under another name counts seconds. A bare `timeout` under 1000
/// does too: nobody waits less than a second on purpose, and shells and Python taught seconds.
fn in_seconds(name: &str, value: &Value) -> bool {
    let name = name.to_ascii_lowercase();
    name.ends_with("seconds")
        || name.ends_with("_s")
        || (name == "timeout" && value.as_f64().is_some_and(|number| number < 1000.0))
}

fn settle_value(value: &mut Value, schema: &Value) {
    match value {
        Value::Number(number) => {
            let Some(given) = number.as_f64() else {
                return;
            };
            let bound = |key: &str| schema.get(key).and_then(Value::as_f64);
            let held = match (bound("minimum"), bound("maximum")) {
                (Some(least), _) if given < least => least,
                (_, Some(most)) if given > most => most,
                _ => return,
            };
            *value = if held.fract() == 0.0 && (number.is_i64() || number.is_u64()) {
                json!(held as i64)
            } else {
                json!(held)
            };
        }
        Value::String(text) if schema.get("type") == Some(&json!("array")) => {
            *value = json!([std::mem::take(text)]);
            settle_value(value, schema);
        }
        Value::Array(items)
            if schema.get("type") == Some(&json!("string"))
                && items.len() == 1
                && items.first().is_some_and(Value::is_string) =>
        {
            *value = items.remove(0);
        }
        Value::String(text) => {
            let Some(choices) = schema.get("enum").and_then(Value::as_array) else {
                return;
            };
            if choices.iter().any(|choice| choice == text.as_str()) {
                return;
            }
            let wanted = spelled(text);
            if let Some(found) = choices
                .iter()
                .filter_map(Value::as_str)
                .find(|choice| spelled(choice) == wanted)
            {
                *text = found.to_owned();
            }
        }
        Value::Array(items) => {
            if let Some(schema) = schema.get("items") {
                items.iter_mut().for_each(|item| settle_value(item, schema));
            }
        }
        Value::Object(fields) => {
            let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
                return;
            };
            for (name, field) in fields.iter_mut() {
                if let Some(schema) = properties.get(name) {
                    settle_value(field, schema);
                }
            }
        }
        _ => {}
    }
}

/// A listed value as agents may write it: in any case, with `-`, `_` or a space between words,
/// or as an operator such as `=` and `!=`.
pub(super) fn spelled(value: &str) -> String {
    let words: String = value
        .chars()
        .filter(|letter| !matches!(letter, '-' | '_' | ' '))
        .flat_map(char::to_lowercase)
        .collect();
    match words.as_str() {
        "=" | "==" | "eq" | "is" => "equals".into(),
        "!=" | "<>" | "ne" | "neq" | "isnot" => "notequals".into(),
        _ => words,
    }
}

fn distance(from: &str, to: &str) -> usize {
    let to: Vec<char> = to.chars().collect();
    let mut previous: Vec<usize> = (0..=to.len()).collect();
    for (row, left) in from.chars().enumerate() {
        let mut current = vec![row + 1];
        for ((right, diagonal), above) in to.iter().zip(&previous).zip(previous.iter().skip(1)) {
            let before = current.last().copied().unwrap_or_default();
            let substitute = diagonal + usize::from(left != *right);
            current.push(substitute.min(above + 1).min(before + 1));
        }
        previous = current;
    }
    previous.last().copied().unwrap_or_default()
}

/// `label` names the value for nested errors; the top level keeps the wording
/// agents already know, so it is `shown` only once inside an array or object.
fn check(value: &Value, schema: &Value, label: &str, shown: bool) -> Result<(), String> {
    let fail = |message: String| {
        if shown {
            format!("{label}: {message}")
        } else {
            message
        }
    };
    let types: Vec<&str> = match schema.get("type") {
        Some(Value::String(expected)) => vec![expected.as_str()],
        Some(Value::Array(expected)) => expected.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    if !types.is_empty() && !types.iter().any(|expected| has_type(value, expected)) {
        let listed: Vec<String> = types
            .iter()
            .map(|expected| format!("'{expected}'"))
            .collect();
        return Err(fail(format!(
            "{} is not of type {}",
            describe(value),
            listed.join(", ")
        )));
    }
    if let (Some(items), Some(values)) = (schema.get("items"), value.as_array()) {
        for (index, item) in values.iter().enumerate() {
            check(item, items, &format!("{label}[{index}]"), true)?;
        }
    }
    if let Some(object) = value.as_object() {
        check_object(object, schema, label, shown)?;
    }
    if let Some(choices) = schema.get("enum").and_then(Value::as_array) {
        if !choices.contains(value) {
            let listed: Vec<String> = choices.iter().map(describe).collect();
            return Err(fail(format!(
                "{} is not one of [{}]",
                describe(value),
                listed.join(", ")
            )));
        }
    }
    if let Some(number) = value.as_f64() {
        if let Some(limit) = schema.get("minimum") {
            if limit.as_f64().is_some_and(|bound| number < bound) {
                return Err(fail(format!(
                    "{} is less than the minimum of {limit}",
                    describe(value)
                )));
            }
        }
        if let Some(limit) = schema.get("maximum") {
            if limit.as_f64().is_some_and(|bound| number > bound) {
                return Err(fail(format!(
                    "{} is greater than the maximum of {limit}",
                    describe(value)
                )));
            }
        }
    }
    if let Some(text) = value.as_str() {
        let length = text.chars().count() as u64;
        if let Some(most) = schema
            .get("maxLength")
            .and_then(Value::as_u64)
            .filter(|most| length > *most)
        {
            return Err(format!(
                "{label} is {length} characters, over the {most} it takes; shorten it and call again"
            ));
        }
        if let Some(least) = schema.get("minLength").and_then(Value::as_u64) {
            if length < least {
                let complaint = if least == 1 {
                    "should be non-empty"
                } else {
                    "is too short"
                };
                return Err(fail(format!("{} {complaint}", describe(value))));
            }
        }
    }
    Ok(())
}

fn check_object(
    object: &Map<String, Value>,
    schema: &Value,
    label: &str,
    shown: bool,
) -> Result<(), String> {
    let properties = schema.get("properties").and_then(Value::as_object);
    for (name, value) in object {
        let field = format!("{label}.{name}");
        match (
            properties.and_then(|properties| properties.get(name)),
            schema.get("additionalProperties"),
        ) {
            (Some(described), _) => check(value, described, &field, true)?,
            (None, Some(Value::Bool(false))) => {
                return Err(format!("{field}: is not allowed here"))
            }
            (None, Some(rest)) if rest.is_object() => check(value, rest, &field, true)?,
            _ => {}
        }
    }
    for name in schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !object.contains_key(name) {
            let message = format!("'{name}' is a required property");
            return Err(if shown {
                format!("{label}: {message}")
            } else {
                message
            });
        }
    }
    Ok(())
}

fn has_type(value: &Value, expected: &str) -> bool {
    match expected {
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn format_tool_schema(decl: &Value) -> String {
    let name = decl.get("name").and_then(Value::as_str).unwrap_or_default();
    let description = decl
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut out = format!("{name}: {description}");
    let required: Vec<&str> = decl
        .pointer("/inputSchema/required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let properties = decl
        .pointer("/inputSchema/properties")
        .and_then(Value::as_object);
    let Some(properties) = properties.filter(|props| !props.is_empty()) else {
        out.push_str("\nParameters: none");
        return out;
    };
    if required.is_empty() {
        out.push_str("\nParameters:");
    } else {
        out.push_str(&format!(
            "\nParameters (required: {}):",
            required.join(", ")
        ));
    }
    for (prop, schema) in properties {
        let mut kind = match schema.get("type") {
            Some(Value::String(kind)) => kind.clone(),
            Some(Value::Array(kinds)) => kinds
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("|"),
            _ => String::new(),
        };
        if kind == "array" {
            if let Some(item_type) = schema.pointer("/items/type").and_then(Value::as_str) {
                kind = format!("{item_type}[]");
            }
        }
        if let Some(choices) = schema.get("enum").and_then(Value::as_array) {
            let listed: Vec<String> = choices
                .iter()
                .map(|choice| match choice.as_str() {
                    Some(text) => text.to_owned(),
                    None => choice.to_string(),
                })
                .collect();
            if !listed.is_empty() {
                if !kind.is_empty() {
                    kind.push_str(", ");
                }
                kind.push_str(&format!("one of: {}", listed.join(", ")));
            }
        }
        if required.contains(&prop.as_str()) {
            if !kind.is_empty() {
                kind.push_str(", ");
            }
            kind.push_str("required");
        }
        let desc = schema
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let meta = if kind.is_empty() {
            String::new()
        } else {
            format!(" ({kind})")
        };
        if desc.is_empty() {
            out.push_str(&format!("\n- {prop}{meta}"));
        } else {
            out.push_str(&format!("\n- {prop}{meta}: {desc}"));
        }
    }
    out
}

fn describe(value: &Value) -> String {
    match value {
        Value::String(text) => format!("'{text}'"),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        other => other.to_string(),
    }
}
