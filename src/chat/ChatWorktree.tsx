import { prettyPath } from "../lib/paths";
import type { AgentWorktree } from "../state/types";
import { IconGit } from "../ui/Icons";
import type { WorktreeSwitchState } from "./worktreeSwitch";
import "../styles/chat/worktree.css";

type ShownSwitch = Exclude<WorktreeSwitchState, { kind: "hidden" }>;

function titleOf(state: ShownSwitch): string {
    switch (state.kind) {
        case "choosing":
            return state.on
                ? "The first message starts this chat in a new git worktree on its own branch. Click to stay in the project folder."
                : "Click to start this chat in a new git worktree on its own branch.";
        case "preparing":
            return `${state.step}…`;
        case "in":
            return `Working in worktree ${state.branch} at ${state.path}`;
    }
}

/** Dressed as the YOLO switch beside it, so the two read as one row of chat settings. */
export function WorktreeToggle({ state, onToggle }: { state: ShownSwitch; onToggle: () => void }) {
    const on = state.kind === "in" || state.kind === "preparing" || (state.kind === "choosing" && state.on);
    return (
        <button
            type="button"
            className={`yolo-toggle worktree-toggle${on ? " on" : ""}`}
            aria-pressed={on}
            disabled={state.kind !== "choosing"}
            title={titleOf(state)}
            onClick={onToggle}>
            <span className="yolo-glyph" aria-hidden="true">
                <IconGit size={12} />
            </span>
            {state.kind === "in" ? <span className="worktree-branch">{state.branch}</span> : <span>worktree</span>}
        </button>
    );
}

export function WorktreeNote({ step, worktree, home }: { step: string | null; worktree: AgentWorktree | undefined; home: string }) {
    if (step !== null)
        return (
            <div className="chat-worktree-note" role="status">
                <span className="chat-activity-loader" aria-hidden="true" />
                <span>{step}…</span>
            </div>
        );
    if (!worktree) return null;
    return (
        <div className="chat-worktree-note" role="note">
            <IconGit size={12} />
            <span>
                Working in worktree <code>{worktree.branch}</code>
            </span>
            <span className="chat-worktree-path" title={worktree.path}>
                {prettyPath(worktree.path, home)}
            </span>
        </div>
    );
}
