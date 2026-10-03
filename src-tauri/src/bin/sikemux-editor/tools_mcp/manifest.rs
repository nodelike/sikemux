//! The tool surface, compiled in from browser/tools.json so the sidecar and the
//! app's handlers can never disagree about what exists.

use serde::Deserialize;
use serde_json::{json, Map, Value};

const MANIFEST: &str = include_str!("../../../../../browser/tools.json");
const GUIDE: &str = include_str!("../../../../../browser/SIKEMUX_GUIDE.md");

/// A tool from browser/tools.json, or one a plugin offers, which the app
/// describes in the same shape.
#[derive(Deserialize)]
pub struct Tool {
    pub name: String,
    pub method: String,
    description: String,
    properties: Map<String, Value>,
    required: Vec<String>,
    /// Set on tools listed only where they can work, such as `simulator` for
    /// the iOS Simulator tools on a Mac with Xcode.
    #[serde(default)]
    pub offered: Option<String>,
}

#[derive(Deserialize)]
struct Guide {
    file: String,
    name: String,
    description: String,
}

#[derive(Deserialize)]
pub struct Manifest {
    guide: Guide,
    tools: Vec<Tool>,
}

impl Manifest {
    pub fn load() -> Self {
        let manifest: Manifest =
            serde_json::from_str(MANIFEST).expect("browser/tools.json is a valid tool manifest");
        assert_eq!(
            manifest.guide.file, "SIKEMUX_GUIDE.md",
            "browser/tools.json names a guide this binary did not compile in"
        );
        manifest
    }

    /// Leaves out the simulator tools unless the app said this Mac can run them.
    pub fn offering(mut self, simulator: bool) -> Self {
        if !simulator {
            self.tools.retain(|tool| tool.offered.is_none());
        }
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

    /// The guide's opening, which every agent reads, or one topic of it, or
    /// all of it for `"all"`.
    pub fn guide_text(&self, topic: Option<&str>) -> Result<&'static str, String> {
        let sections = guide_topics();
        match topic {
            None => Ok(sections
                .first()
                .map_or(GUIDE, |(_, start)| &GUIDE[..*start])),
            Some(ALL_TOPICS) => Ok(GUIDE),
            Some(wanted) => sections
                .iter()
                .enumerate()
                .find(|(_, (name, _))| *name == wanted)
                .map(|(index, (_, start))| {
                    let end = sections
                        .get(index + 1)
                        .map_or(GUIDE.len(), |(_, next)| *next);
                    GUIDE[*start..end].trim_end()
                })
                .ok_or_else(|| {
                    let names: Vec<&str> = sections.iter().map(|(name, _)| *name).collect();
                    format!(
                        "No guide topic '{wanted}'. Topics: {}, or {ALL_TOPICS}",
                        names.join(", ")
                    )
                }),
        }
    }

    pub fn instructions(&self) -> String {
        let mut instructions = format!(
            "Sikemux drives the person's open project and this agent's browser tabs. Call {} before the first task launch or browser click. \
             When the person asks you to open, show or preview a page, use browser_navigate: it opens on your desk beside them, not in their own browser. \
             Desk tabs run the same WebKit as Sikemux and Safari, so check web pages there rather than in headless Chromium. \
             The browser tools used most are browser_navigate, browser_state, browser_find, browser_click, browser_type, browser_wait and browser_screenshot; load them together.",
            self.guide.name
        );
        if self.offers_simulator() {
            instructions.push_str(
                " To check an iOS app, sim_attach a simulator, read it with sim_state and act with sim_tap, sim_type and sim_swipe; load them together. \
                 sim_attach shows the device live on your desk beside the person, as browser_navigate does a page, so do not look for Simulator.app or open screenshots elsewhere to show it.",
            );
        }
        instructions
    }

    pub fn tool(&self, name: &str) -> Option<&Tool> {
        self.tools.iter().find(|tool| tool.name == name)
    }

    /// Whether a name is already taken here, so a plugin tool cannot shadow it.
    pub fn declares(&self, name: &str) -> bool {
        name == self.guide.name || self.tool(name).is_some()
    }

    pub fn declarations(&self) -> Vec<Value> {
        let mut declared: Vec<Value> = self.tools.iter().map(Tool::declaration).collect();
        declared.push(declaration(
            &self.guide.name,
            &self.guide.description,
            &guide_properties(),
            &[],
        ));
        declared
    }
}

const ALL_TOPICS: &str = "all";

/// Each `## name: Title` heading of the guide, with where its section starts.
/// Headings without a name belong to the opening every agent reads.
fn guide_topics() -> Vec<(&'static str, usize)> {
    let mut start = 0;
    let mut topics = Vec::new();
    for line in GUIDE.split_inclusive('\n') {
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

fn guide_properties() -> Map<String, Value> {
    let mut topics: Vec<&str> = guide_topics().into_iter().map(|(name, _)| name).collect();
    topics.push(ALL_TOPICS);
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
    pub fn declaration(&self) -> Value {
        declaration(
            &self.name,
            &self.description,
            &self.properties,
            &self.required,
        )
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

    /// The property an agent most likely meant by a name this tool lacks.
    fn closest(&self, name: &str) -> Option<&str> {
        const SAME_MEANING: &[(&str, &[&str])] = &[
            (
                "script",
                &["expression", "code", "js", "javascript", "function"],
            ),
            ("selector", &["query", "css"]),
            ("tail", &["lines"]),
            ("taskId", &["name", "task", "id"]),
            ("deltaY", &["y", "dy", "amount", "pixels"]),
            ("url", &["href", "link"]),
            ("text", &["value", "content"]),
            ("index", &["element", "ref", "number"]),
        ];
        let lowered = name.to_ascii_lowercase();
        let known = |candidate: &str| self.properties.contains_key(candidate);
        SAME_MEANING
            .iter()
            .find(|(meant, others)| known(meant) && others.contains(&lowered.as_str()))
            .map(|(meant, _)| *meant)
            .or_else(|| {
                self.properties
                    .keys()
                    .map(|candidate| {
                        (
                            candidate,
                            distance(&lowered, &candidate.to_ascii_lowercase()),
                        )
                    })
                    .filter(|(_, apart)| *apart <= 2)
                    .min_by_key(|(_, apart)| *apart)
                    .map(|(candidate, _)| candidate.as_str())
            })
    }
}

fn distance(from: &str, to: &str) -> usize {
    let to: Vec<char> = to.chars().collect();
    let mut previous: Vec<usize> = (0..=to.len()).collect();
    for (row, left) in from.chars().enumerate() {
        let mut current = vec![row + 1];
        for (column, right) in to.iter().enumerate() {
            let substitute = previous[column] + usize::from(left != *right);
            current.push(
                substitute
                    .min(previous[column + 1] + 1)
                    .min(current[column] + 1),
            );
        }
        previous = current;
    }
    previous[to.len()]
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
        if schema
            .get("maxLength")
            .and_then(Value::as_u64)
            .is_some_and(|most| length > most)
        {
            return Err(fail(format!("{} is too long", describe(value))));
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

/// The fields inside an object argument, when the schema describes them.
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

fn describe(value: &Value) -> String {
    match value {
        Value::String(text) => format!("'{text}'"),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        other => other.to_string(),
    }
}
