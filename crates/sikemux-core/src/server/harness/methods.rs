//! What the core must know of a tool method before running it, one row per method or family.
//! Running it is `dispatch`.

use crate::cli::protocol::{
    COMMIT_FOOTER_METHOD, COMMIT_MADE_METHOD, GIT_GUARD_METHOD, PLUGINS_CHANGED_METHOD,
    SHIP_CHECK_METHOD, SHIP_RAN_METHOD, SIM_CANCEL_METHOD, SIM_OFFERED_METHOD,
    TOOLS_MANIFEST_METHOD,
};

pub(super) struct Method {
    /// One method, or with a final `.` every method under that name not matched earlier.
    name: &'static str,
    /// Acts as the agent calling it, or with a sign-in or a view the caller does not hold
    /// itself, so a call from outside every agent's processes is refused, not answered for no one.
    pub(super) as_agent: bool,
    /// Could show a reviewer Auto started blind the builder's side.
    pub(super) blind_refused: bool,
    /// The window answers it from the desk of the agent calling.
    pub(super) pinned: bool,
    /// How often it was called and worked is kept. Not for plumbing agents never ask for.
    pub(super) counted: bool,
    /// What the window is needed for, in the words that end "to ...".
    pub(super) purpose: &'static str,
}

const OTHER: Method = Method {
    name: "",
    as_agent: false,
    blind_refused: false,
    pinned: false,
    counted: true,
    purpose: "use this tool",
};

const fn acting(name: &'static str) -> Method {
    Method {
        name,
        as_agent: true,
        ..OTHER
    }
}

const fn acting_unseen_by_blind(name: &'static str) -> Method {
    Method {
        blind_refused: true,
        ..acting(name)
    }
}

const fn on_desk(name: &'static str, purpose: &'static str) -> Method {
    Method {
        pinned: true,
        purpose,
        ..acting(name)
    }
}

const fn plumbing(method: Method) -> Method {
    Method {
        counted: false,
        ..method
    }
}

const fn plumbing_for_anyone(name: &'static str) -> Method {
    plumbing(Method { name, ..OTHER })
}

const SIMULATOR: &str = "drive the iOS Simulator";

const METHODS: &[Method] = &[
    acting_unseen_by_blind("agent.spawn"),
    acting_unseen_by_blind("agent.sessions"),
    acting_unseen_by_blind("agent.wrote"),
    acting("agent."),
    acting_unseen_by_blind("board."),
    acting("memory."),
    plumbing(acting(COMMIT_FOOTER_METHOD)),
    plumbing(acting(COMMIT_MADE_METHOD)),
    plumbing(acting(GIT_GUARD_METHOD)),
    acting("git."),
    acting("review."),
    on_desk("browser.", "use the browser"),
    plumbing(on_desk(SIM_OFFERED_METHOD, SIMULATOR)),
    plumbing(on_desk(SIM_CANCEL_METHOD, SIMULATOR)),
    on_desk("sim.", SIMULATOR),
    acting("plugins.call"),
    plumbing_for_anyone("plugins.tools"),
    plumbing_for_anyone(PLUGINS_CHANGED_METHOD),
    plumbing_for_anyone(TOOLS_MANIFEST_METHOD),
    plumbing_for_anyone(SHIP_CHECK_METHOD),
    plumbing_for_anyone(SHIP_RAN_METHOD),
    Method {
        purpose: "read its console",
        ..acting("app.console")
    },
    Method {
        name: "workspace.inspect",
        purpose: "inspect the workspace",
        ..OTHER
    },
    Method {
        name: "ui.open",
        purpose: "open things in it",
        ..OTHER
    },
];

pub(super) fn of(method: &str) -> &'static Method {
    METHODS
        .iter()
        .find(|row| match row.name.strip_suffix('.') {
            Some(_) => method.starts_with(row.name),
            None => method == row.name,
        })
        .unwrap_or(&OTHER)
}

/// The tool an agent knows `method` by.
pub(super) fn tool_word(method: &str) -> String {
    method.replace('.', "_")
}
