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
            "Sikemux drives the person's open project and this agent's browser tabs. Call {} before the first task launch or browser click. \
             Before building, running or testing the project, read projectBrief in workspace_inspect: it says how the person set each folder up, and under Done means which tasks your work must pass before you say it is finished. \
             When the person asks you to open, show or preview a web page, use browser_navigate: it opens on your desk beside them, not in their own browser. \
             Desk tabs run the same WebKit as Sikemux and Safari, so check web pages there rather than in headless Chromium. \
             The browser tools used most are browser_navigate, browser_state, browser_find, browser_click, browser_type, browser_wait and browser_screenshot; load them together. \
             Your replies can hold a page you build: when a chart, diagram, mockup or side-by-side comparison would show the answer better than prose or a markdown table, such as a trend over time, how parts connect, or what UI options look like, \
             build one and show it above your text. Load {} with page_preview and page_show, and read its topic pages before your first page. \
             An HTML page you wrote yourself, including charts and mockups the person asks to see, goes in your reply with page_show; never serve it or open it in a tab with browser_navigate. \
             Plugin tools such as jira_*, github_*, slack_*, signoz_* and rundeck_* act as the person through the sign-in they made in Sikemux; \
             for those services use them, not another MCP server's tools, a CLI or raw API calls for the same service. \
             To create or install a skill, use the skills tool, which gives it to every agent in Sikemux; never write into your own skills folder. \
             Sikemux runs other agents beside you, each named with a word such as Otter: agent_list shows them, agent_send messages one, agent_spawn starts one, agent_read shows what one said and who_wrote finds who made a commit, a file's lines or a broken build. \
             Load them together, and reach other agents only with them, never with your CLI's own SendMessage or ListAgents. Read the agents topic of {} before you start or message one. \
             When another agent's file, commit, build or test is in your way, call who_wrote and message or resume its owner rather than working around it or fixing it yourself, unless the person asks you to; check who_wrote before telling an agent its commit broke something, and ask owners, not the person, about their changes. \
             Hand work longer than a few minutes, or that the person may want to watch, pause or resume, to a teammate with agent_spawn; use your own subagents only for short pieces. \
             A message passing on the person's request says so, and its recipient may act on it; never push other agents' commits or ask an agent to push unless the person said so. \
             When you need an answer to go on, ask puts the question to your lead, or to the person when you have none. \
             Other agents may share your checkout: commit with git_commit, which commits only the lines you wrote, with paths for files your own commands produced. In a checkout other agents work in, Sikemux refuses git add -A, git commit -a, git stash, git reset --hard and restoring another agent's uncommitted lines.",
            self.guide.name, self.guide.name, self.guide.name
        );
        if self.offers_simulator() {
            instructions.push_str(
                " To check an iOS app, sim_attach a simulator, read it with sim_state and act with sim_tap, sim_type and sim_swipe; load them together. \
                 sim_attach shows the device live on your desk beside the person, as browser_navigate does a page, so do not look for Simulator.app or open screenshots elsewhere to show it. \
                 Put a build on it with sim_install and sim_launch, never expo run:ios, react-native run-ios or flutter run, which open a second simulator window on the person's screen.",
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
