//! The agents' built-in tools and the guide they read, as this build declares
//! them. The core serves them to agents' tool servers, so a tool server that
//! outlived an update still offers what the running Sikemux handles.

use std::sync::LazyLock;

use serde_json::{json, Value};

pub const TOOLS: &str = include_str!("../../../../tools/tools.json");
pub const GUIDE: &str = include_str!("../../../../tools/SIKEMUX_GUIDE.md");

static SERVED: LazyLock<Result<Value, String>> = LazyLock::new(|| {
    let manifest: Value = serde_json::from_str(TOOLS).map_err(|error| error.to_string())?;
    Ok(json!({ "manifest": manifest, "guide": GUIDE }))
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
    static GUIDE_NAME: LazyLock<String> = LazyLock::new(|| {
        serde_json::from_str::<Value>(TOOLS)
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
    instructions_for(&GUIDE_NAME, simulator)
}
