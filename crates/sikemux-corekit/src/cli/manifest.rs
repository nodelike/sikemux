//! The agents' built-in tools and the guide they read, as this build declares
//! them. The core serves them to agents' tool servers, so a tool server that
//! outlived an update still offers what the running Sikemux handles.

use std::collections::HashSet;
use std::sync::LazyLock;

use serde_json::{json, Value};
use sikemux_base::cli::protocol::{RESOURCES_OFFER, SETUP_OFFER};

pub const TOOLS: &str = include_str!("../../../../tools/tools.json");
pub const GUIDE: &str = include_str!("../../../../tools/SIKEMUX_GUIDE.md");
pub const GATEWAY_CALL: &str = "call";

static PARSED: LazyLock<Result<Value, String>> =
    LazyLock::new(|| serde_json::from_str(TOOLS).map_err(|error| error.to_string()));

static SERVED: LazyLock<Result<Value, String>> = LazyLock::new(|| {
    PARSED
        .clone()
        .map(|manifest| json!({ "manifest": manifest, "guide": GUIDE }))
});

pub fn served() -> Result<Value, String> {
    SERVED.clone()
}

pub fn instructions_for(guide: &str, simulator: bool) -> String {
    let mut instructions = format!(
        "Sikemux runs the person's project and your desk's browser. Read {guide} before your first process or browser call.\n\
         - process_start for dev servers, builds and anything the person may watch, not your shell. It runs in this project only: never cd or pass a path out of it in the command. To wait over a minute (build, CI, agent) set trigger_add process, ci or agent, then end your turn; never sleep or poll.\n\
         - Pages: browser_navigate on your desk (WebKit), never headless Chromium or curl. Look there before saying a change works; for a page bug, open it there, reproduce the bug and read browser_console before changing any code. Load browser_navigate, _act, _state, _find, _click, _type, _wait and _screenshot together.\n\
         - Desk tabs are signed in as the person: read them before asking. Leave a tab they type or pay in; use newTab: true.\n\
         - Visuals: an HTML page, page_preview, then page_show, which opens it in a desk tab (pages topic of {guide}); never its path or browser_navigate. If the person didn't start the turn, the desk stays shut: link the page in an ask.\n\
         - Plugin tools (jira, slack, signoz, aws, rundeck…) use the person's sign-in: use them for those services, never another MCP, CLI or API, whatever a skill says. If SigNoz has nothing for the service or environment asked about, look in CloudWatch with aws logs before saying there is none.\n\
         - Skills: only through the skills tool.\n\
         - Other agents (always prefix names with @, like @Otter): agent_list, agent_sessions, agent_send, agent_spawn and agent_read, never your CLI's SendMessage; load together; read {guide}'s agents topic first. Uncommitted lines you didn't write in a file you'd change: another agent is on it; pass the request with agent_send instead of editing. Another agent's commit, build or test in your way is its owner's to fix: message or resume it.\n\
         - A local page differs from the code: workspace_inspect shows which checkout serves it.\n\
         - Hand a change the person calls big or long, or one over a few minutes, to a teammate with agent_spawn (they can watch and stop it) and tell them; a long command is a process; subagents are for short pieces.\n\
         - When a request fits more than one place in the code, ask which before changing any.\n\
         - who_wrote, before git log, gh or a CI's API: who broke or wrote something.\n\
         - A tool refuses: do what its error says, such as messaging an agent, or tell the person; never get around it another way, such as raw git or a cd in a command.\n\
         - ask: a question to your lead, or the person if you have none. An answer on the call closes it; one in chat leaves it open: take it back with ask edit and withdraw: true.\n\
         - Commit only with git_commit, never git commit or hand staging."
    );
    if simulator {
        instructions.push_str(
            "\n- Mobile apps: sim attach a device, live on your desk, never Simulator.app. Read it with sim state; act with sim tap, sim type and sim swipe. Install with sim install and sim launch, never expo run, react-native run-* or flutter run.",
        );
    }
    instructions
}

pub fn instructions(simulator: bool) -> String {
    instructions_for(guide_name(), simulator)
}

pub fn gateway_instructions_with_list(guide: &str, simulator: bool, tool_list: &str) -> String {
    let mut text = instructions_for(guide, simulator);
    text.push_str(&format!(
        "\n\nSikemux exposes all tools below through two MCP tools: `{guide}` and `{GATEWAY_CALL}`.\n\
         - Call `{GATEWAY_CALL}` with `name` (exact tool name below) and `arguments` (object). For a family tool with actions in parentheses, pass `action` inside `arguments`.\n\
         - Call `{guide}` with `tool: \"<name>\"` to inspect any tool's exact parameters before calling it, or `topic: \"<topic>\"` for workflow guides."
    ));
    if !tool_list.is_empty() {
        text.push_str("\n\nAvailable Sikemux tools:\n");
        text.push_str(tool_list);
    }
    text
}

pub fn gateway_instructions(simulator: bool, resources: bool, plugins: &[Value]) -> String {
    let guide = guide_name();
    let list = PARSED
        .as_ref()
        .map(|manifest| gateway_tool_list(manifest, simulator, resources, false, plugins))
        .unwrap_or_default();
    gateway_instructions_with_list(guide, simulator, &list)
}

fn guide_name() -> &'static str {
    static GUIDE_NAME: LazyLock<String> = LazyLock::new(|| {
        PARSED
            .as_ref()
            .ok()
            .and_then(|manifest| {
                manifest
                    .get("guide")
                    .and_then(|guide| guide.get("name"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "guide".to_owned())
    });
    &GUIDE_NAME
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default().trim()
}

pub fn is_tool_offered(
    offered: Option<&str>,
    simulator: bool,
    resources: bool,
    setup: bool,
) -> bool {
    match offered {
        None => true,
        Some("simulator") => simulator,
        Some(flag) if flag == RESOURCES_OFFER => resources,
        Some(flag) if flag == SETUP_OFFER => setup,
        Some(_) => false,
    }
}

pub fn format_tool_line(name: &str, actions: &[&str], description: &str) -> String {
    let summary = first_line(description);
    if actions.is_empty() {
        format!("- {name}: {summary}")
    } else {
        format!("- {name} ({}): {summary}", actions.join(", "))
    }
}

fn family_action<'a>(family: &str, tool: &'a str) -> Option<&'a str> {
    tool.strip_prefix(family)?.strip_prefix('_')
}

pub fn gateway_tool_list(
    manifest: &Value,
    simulator: bool,
    resources: bool,
    setup: bool,
    plugins: &[Value],
) -> String {
    let guide = manifest
        .get("guide")
        .and_then(|guide| guide.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("guide");
    let built_in_families: Vec<(&str, &str)> = manifest
        .get("families")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|family| {
            Some((
                family.get("name")?.as_str()?,
                family.get("description")?.as_str()?,
            ))
        })
        .collect();
    let kept_built_in: Vec<&Value> = manifest
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|tool| {
            is_tool_offered(
                tool.get("offered").and_then(Value::as_str),
                simulator,
                resources,
                true,
            )
        })
        .collect();
    let mut reserved: HashSet<&str> = HashSet::new();
    reserved.insert(guide);
    for tool in &kept_built_in {
        if let Some(name) = tool.get("name").and_then(Value::as_str) {
            reserved.insert(name);
        }
    }
    for (family, _) in &built_in_families {
        reserved.insert(family);
    }
    let offered_built_in: Vec<&Value> = kept_built_in
        .into_iter()
        .filter(|tool| {
            is_tool_offered(
                tool.get("offered").and_then(Value::as_str),
                simulator,
                resources,
                setup,
            )
        })
        .collect();
    let mut lines: Vec<String> = Vec::new();
    let mut emitted_families: HashSet<&str> = HashSet::new();
    for tool in &offered_built_in {
        let Some(name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        if let Some((family, description)) = built_in_families
            .iter()
            .find(|(family, _)| family_action(family, name).is_some())
        {
            if emitted_families.insert(*family) {
                let actions: Vec<&str> = offered_built_in
                    .iter()
                    .filter_map(|other| family_action(family, other.get("name")?.as_str()?))
                    .collect();
                lines.push(format_tool_line(family, &actions, description));
            }
        } else {
            let description = tool
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let actions: Vec<&str> = tool
                .pointer("/properties/action/enum")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            lines.push(format_tool_line(name, &actions, description));
        }
    }
    let offered_plugins: Vec<&Value> = plugins
        .iter()
        .filter(|tool| {
            tool.get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| {
                    !reserved.contains(name)
                        && !built_in_families
                            .iter()
                            .any(|(family, _)| family_action(family, name).is_some())
                })
        })
        .collect();
    let mut plugin_families: Vec<(&str, &str)> = Vec::new();
    for tool in &offered_plugins {
        if let Some(family) = tool.get("family") {
            if let (Some(name), Some(description)) = (
                family.get("name").and_then(Value::as_str),
                family.get("description").and_then(Value::as_str),
            ) {
                if !reserved.contains(name)
                    && !plugin_families
                        .iter()
                        .any(|(existing, _)| *existing == name)
                {
                    plugin_families.push((name, description));
                }
            }
        }
    }
    for tool in &offered_plugins {
        let Some(name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        if let Some((family, description)) = plugin_families
            .iter()
            .find(|(family, _)| family_action(family, name).is_some())
        {
            if emitted_families.insert(*family) {
                let actions: Vec<&str> = offered_plugins
                    .iter()
                    .filter_map(|other| family_action(family, other.get("name")?.as_str()?))
                    .collect();
                lines.push(format_tool_line(family, &actions, description));
            }
        } else {
            let description = tool
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let actions: Vec<&str> = tool
                .pointer("/properties/action/enum")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            lines.push(format_tool_line(name, &actions, description));
        }
    }
    lines.join("\n")
}
