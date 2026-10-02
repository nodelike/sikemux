import { copyText } from "../lib/clipboard";
import type { CtxItem } from "../rail/FileTree";
import * as cmd from "../state/commands";
import { promptDialog } from "../state/dialog";
import { notify, reportError } from "../state/toast";
import type { Agent, Session } from "../state/types";

/**
 * The menu an agent's tab, its header and its rail row open. `others` are the agents "Close Others" closes.
 * `rename` edits the name in place where the name is on screen; without it, Rename asks in a dialog.
 */
export function agentMenu(
    agent: Agent,
    others: Agent[],
    session: Session,
    hints: { close: string; permissions: string },
    rename: () => void = () => void renameAgentPrompt(agent),
): CtxItem[] {
    const link = cmd.agentLink(agent, session);
    const items: CtxItem[] = [
        { label: "Rename…", run: rename },
        ...(agent.launchState === "dormant"
            ? [{ label: "Resume", run: () => cmd.selectAgent(agent.id) }]
            : agent.resumeId
              ? [{ label: "Sleep", run: () => cmd.sleepAgent(agent.id) }]
              : []),
        ...(agent.resumeId && agent.launchState !== "dormant"
            ? [{ label: agent.keepAlive ? "Allow Auto-Sleep" : "Keep Alive", run: () => cmd.setAgentKeepAlive(agent.id, !agent.keepAlive) }]
            : []),
        ...(agent.worktree
            ? [
                  { sep: true },
                  {
                      label: "Remove worktree…",
                      run: () => void import("../agents/agentWorktree").then(({ removeAgentWorktree }) => removeAgentWorktree(agent.id)),
                  },
              ]
            : []),
        { sep: true },
        { label: "Close", hint: hints.close, run: () => cmd.closeAgent(agent.id) },
        { label: "Close Others", disabled: others.length === 0, run: () => others.forEach((x) => cmd.closeAgent(x.id)) },
    ];
    if (cmd.agentSupportsSkipPermissions(agent.type)) {
        const skip = agent.permissionMode === "bypass" || agent.skipPermissions === true;
        items.push(
            { sep: true },
            {
                label: skip ? "Disable YOLO Mode" : "Enable YOLO Mode",
                hint: hints.permissions,
                run: () => cmd.toggleAgentSkipPermissions(agent.id),
            },
        );
    }
    items.push(
        { sep: true },
        {
            label: "Copy Link",
            disabled: !link,
            run: () => {
                if (link) void copyText(link).then(() => notify("success", "copied link"), reportError("copy"));
            },
        },
    );
    return items;
}

async function renameAgentPrompt(agent: Agent): Promise<void> {
    const title = await promptDialog({ title: "Rename chat", label: "Name", initial: agent.title, confirmLabel: "Rename" });
    if (title) cmd.renameAgent(agent.id, title);
}
