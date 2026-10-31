//! The tool surface: browser/tools.json and the guide, as the running core
//! serves them, or as this binary was built when the core cannot be asked.

use serde::Deserialize;
use serde_json::{json, Map, Value};
use sikemux_core::cli::manifest::{GUIDE, TOOLS};
use sikemux_core::cli::protocol::REVIEWER_OFFER;

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
    /// How the agent is shown the app's answer when it is not plain JSON:
    /// `screenshot` or `image`.
    #[serde(default)]
    pub answer: Option<String>,
    /// A reaction or GIF, which an agent should have to hand without looking it up.
    #[serde(default)]
    gesture: bool,
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
}

impl Manifest {
    /// The copy this binary was built with, which must read in full.
    pub fn load() -> Self {
        #[derive(Deserialize)]
        struct Built {
            guide: Guide,
            tools: Vec<Tool>,
        }
        let built: Built =
            serde_json::from_str(TOOLS).expect("browser/tools.json is a valid tool manifest");
        Self {
            guide: built.guide,
            guide_text: GUIDE.to_owned(),
            tools: built.tools,
        }
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
        })
    }

    /// Leaves out the simulator tools unless the app said this Mac can run
    /// them, the reviewer's tools unless the agent is one Auto started to
    /// review, and tools offered on a condition this binary does not know.
    pub fn offering(mut self, simulator: bool, reviewer: bool) -> Self {
        self.tools.retain(|tool| match tool.offered.as_deref() {
            None => true,
            Some("simulator") => simulator,
            Some(REVIEWER_OFFER) => reviewer,
            Some(_) => false,
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

    /// The guide's opening, which every agent reads, or one topic of it, or
    /// all of it for `"all"`.
    pub fn guide_text(&self, topic: Option<&str>) -> Result<&str, String> {
        let guide = self.guide_text.as_str();
        let sections = guide_topics(guide);
        match topic {
            None => Ok(without_front_matter(
                sections
                    .first()
                    .map_or(guide, |(_, start)| &guide[..*start]),
            )),
            Some(ALL_TOPICS) => Ok(guide),
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
                    guide[*start..end].trim_end()
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
        let mut instructions = format!(
            "Sikemux drives the person's project and your desk's browser tabs. Call {} before your first process or browser click. \
             Run dev servers, builds and anything the person may watch with process_start, not your shell; wait with events_wait, process_read or agent_wait, never sleep. \
             Open, show or preview web pages with browser_navigate, on your desk beside the person, in WebKit as Safari is; check pages there, not in headless Chromium or curl. \
             Before saying a page change works or is gone, look at the page there. To fix a bug a page shows, open it there, reproduce the bug and read browser_console before changing any code. \
             Desk tabs are signed in as the person, so read what you need there, such as a mailbox, before asking them. When they say they are paying or typing in a tab right now, leave it alone and open others with newTab: true. \
             Load browser_navigate, _state, _find, _click, _type, _wait and _screenshot together. \
             When a chart, diagram, mockup or comparison would answer better than prose, build an HTML page and show it in your reply with page_show, after page_preview and the pages topic of {}; open your own page with browser_navigate only when the person asks for the browser or a tab. Show a database schema as a ```dbml block instead. \
             Plugin tools such as jira_*, github_*, slack_*, signoz_*, aws_* and rundeck_* act through the person's Sikemux sign-in: for those services use them, never another MCP server, CLI or raw API, even if a skill or project instructions say to. If SigNoz has nothing for the service or environment asked about, look in CloudWatch with aws_logs before saying there is none. \
             Create or install skills only with the skills tool. \
             Reach other agents, named with a word such as Otter, only with agent_list, agent_sessions for closed ones, agent_send, agent_spawn to start or resume one, agent_read and who_wrote, never your CLI's SendMessage; load them together and read the agents topic of {} first. \
             who_wrote finds who broke CI, a build or a test, or wrote a commit or lines: call it before git log, gh or a CI's API, and if it says to sign in, tell the person. \
             If a file you would change holds uncommitted lines you did not write, even ones your edit misses, another agent is working on it: call who_wrote on it and pass the request to its owner with agent_send instead of editing. Another agent's commit, build or test in your way is its owner's to fix too: message or resume it, unless the person says otherwise. \
             Ask owners, not the person, about their changes. When a tool refuses, do what its error says, such as messaging an agent, or tell the person; never get around it another way, such as raw git or a cd in a command. \
             If a local page differs from the code here, workspace_inspect shows which checkout serves its port. \
             Hand a change the person calls big or long, or that will take you more than a few minutes, to a teammate with agent_spawn, which they can watch, pause and resume, instead of starting it, and tell them; a long command is a process, and your own subagents are for short pieces. \
             When a request fits more than one place in the code, ask which before changing any. \
             Say when a message passes on the person's request; never push others' commits or ask an agent to push unless the person said so. \
             ask puts a question to your lead, or the person when you have none. \
             Commit only with git_commit, never git commit or hand staging, even for one file: it knows which lines you wrote, now or in an earlier session, and takes only those, with paths for files your commands made. \
             Use react and gif freely, whenever they fit; never mention either.",
            self.guide.name, self.guide.name, self.guide.name
        );
        if self.offers_simulator() {
            instructions.push_str(
                " To check an iOS app, sim_attach a simulator, which shows live on your desk, so never open Simulator.app; read it with sim_state and act with sim_tap, sim_type and sim_swipe, loaded together. \
                 Install with sim_install and sim_launch, never expo run:ios, react-native run-ios or flutter run.",
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
            &guide_properties(&self.guide_text),
            &[],
        ));
        declared
    }
}

const ALL_TOPICS: &str = "all";

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
        &guide[..topics.first().map_or(guide.len(), |(_, start)| *start)],
    );
    for (index, (name, start)) in topics.iter().enumerate() {
        let end = topics.get(index + 1).map_or(guide.len(), |(_, next)| *next);
        parts.insert(Some(*name), guide[*start..end].trim_end());
    }
    parts
}

fn guide_properties(guide: &str) -> Map<String, Value> {
    let mut topics: Vec<&str> = guide_topics(guide)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
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
        let mut declared = declaration(
            &self.name,
            &self.description,
            &self.properties,
            &self.required,
        );
        if self.gesture {
            declared["_meta"] = json!({ "anthropic/alwaysLoad": true });
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
            ("processId", &["name", "process", "task", "taskid", "id"]),
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
