//! The tool surface: tools/tools.json and the guide, as the running core
//! serves them, or as this binary was built when the core cannot be asked.

use std::collections::HashSet;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::{json, Map, Value};
use sikemux_core::cli::manifest::{GUIDE, TOOLS};
use sikemux_core::cli::protocol::REVIEWER_OFFER;

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
    /// A GIF, which an agent should have to hand without looking it up.
    #[serde(default)]
    gesture: bool,
    /// How the app shows a call of it, which says whether it changes anything.
    #[serde(default)]
    row: Row,
    /// The tool a plugin offers its tools as, which the core says beside each.
    #[serde(default)]
    family: Option<Family>,
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
        let guide = &self.guide.name;
        let mut instructions = format!(
            "Sikemux runs the person's project and your desk's browser. Read {guide} before your first process or browser call.\n\
             - process_start for dev servers, builds and anything the person may watch, not your shell. It runs in this project only: never cd or pass a path out of it in the command. To wait over a minute (build, CI, agent) set trigger_add process, ci or agent, then end your turn; never sleep or poll.\n\
             - Pages: browser_navigate on your desk (WebKit), never headless Chromium or curl. Look there before saying a change works; for a page bug, open it there, reproduce the bug and read browser_console before changing any code. Load browser_navigate, _act, _state, _find, _click, _type, _wait and _screenshot together.\n\
             - Desk tabs are signed in as the person: read them (a mailbox) before asking. Leave a tab they type or pay in; use newTab: true. grok.com: a new chat, Expert or Auto, not the bot.\n\
             - Visuals: an HTML page, page_preview, then page_show (pages topic of {guide}); browser_navigate it only when the person asks for the browser or a tab. Schemas: ```dbml.\n\
             - Plugin tools (jira, slack, signoz, aws, rundeck…) use the person's sign-in: use them for those services, never another MCP, CLI or API, whatever a skill says. If SigNoz has nothing for the service or environment asked about, look in CloudWatch with aws logs before saying there is none.\n\
             - Skills: only through the skills tool.\n\
             - Other agents, named like Otter: only agent_list, agent_sessions, agent_send, agent_spawn, agent_read and who_wrote, never your CLI's SendMessage; load them together and read the agents topic of {guide} first. Ask owners, not the person, about their work.\n\
             - who_wrote, before git log, gh or a CI's API: who broke or wrote something.\n\
             - Uncommitted lines you didn't write in a file you'd change: another agent is on it. Pass the request to its owner with agent_send instead of editing. Another agent's commit, build or test in your way is its owner's to fix: message or resume it.\n\
             - A tool refuses: do what its error says, such as messaging an agent, or tell the person; never get around it another way, such as raw git or a cd in a command.\n\
             - A local page differs from the code: workspace_inspect shows which checkout serves it.\n\
             - Hand a change the person calls big or long, or that will take you more than a few minutes, to a teammate with agent_spawn, which they can watch, pause and resume, and tell them; a long command is a process, and your subagents are for short pieces.\n\
             - When a request fits more than one place in the code, ask which before changing any.\n\
             - Say when you pass on the person's request. Push others' commits only if the person said so.\n\
             - ask: a question to your lead, or the person if you have none.\n\
             - Commit only with git_commit (just your lines; pass paths for files your commands made), never git commit or hand staging.\n\
             - react and gif freely; never mention them."
        );
        if self.offers_simulator() {
            instructions.push_str(
                "\n- Mobile apps: sim attach a device, live on your desk, never Simulator.app. Read it with sim state; act with sim tap, sim type and sim swipe. Install with sim install and sim launch, never expo run, react-native run-* or flutter run.",
            );
        }
        instructions
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
    pub fn declarations(&self, plugins: &[Tool]) -> Vec<Value> {
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

    /// The arguments as this tool takes them, so a call an agent got nearly right runs rather
    /// than coming back refused: a name it commonly uses for one of this tool's is renamed,
    /// `repo: "owner/name"` is split where the tool takes `owner` and `name`, a listed value
    /// written in another case or spelling is matched, and a number past a bound is held at it.
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
            if let Some((owner, repo)) = self.split_repo(&name, &object[&name]) {
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
            ("text", &["value", "content"]),
            ("index", &["element", "ref", "number"]),
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
                .filter(|(_, apart)| *apart <= 2)
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

fn describe(value: &Value) -> String {
    match value {
        Value::String(text) => format!("'{text}'"),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        other => other.to_string(),
    }
}
