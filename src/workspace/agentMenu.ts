import { createElement } from "react";
import { copyText } from "../lib/clipboard";
import { menuIcon, type CtxItem } from "../rail/FileTree";
import * as cmd from "../state/commands";
import { promptDialog } from "../state/dialog";
import { notify, reportError } from "../state/toast";
import type { Agent, Session } from "../state/types";
import { isYolo } from "../agents/agentLaunch";
import { canBypassPermissions, agentSupportsChat } from "../agents/catalog";
import { openShareDialog } from "../share/macShares";
import { openBuddyPicker } from "../buddies/commands";
import { getState } from "../state/store";
import { AgentBuddy } from "../buddies/Buddy";
import {
    IconClose,
    IconCopy,
    IconFolder,
    IconPencil,
    IconPin,
    IconPower,
    IconRotate,
    IconShare,
    IconShield,
    IconShieldBolt,
    IconTrash,
} from "../ui/Icons";
import { hasChat } from "../state/selectors";

/**
 * The menu an agent's tab, its header and its rail row open. `others` are the agents "Close Others" closes.
 * `retitle` edits the title in place where the title is on screen; without it, Edit Title asks in a dialog.
 */
export function agentMenu(
    agent: Agent,
    others: Agent[],
    session: Session,
    hints: { close: string; permissions: string },
    retitle: () => void = () => void renameAgentPrompt(agent),
): CtxItem[] {
    const link = cmd.agentLink(agent, session);
    const resumeId = agent.resumeId;
    const pinned = getState().pinnedChats.some((pin) => hasChat(agent, pin));
    const items: CtxItem[] = [
        { label: "Rename…", icon: menuIcon(IconPencil), run: () => void renameAgentNamePrompt(agent) },
        { label: "Edit Title…", icon: menuIcon(IconPencil), run: retitle },
        ...(getState().faceStyle.on
            ? [{ label: "Change Buddy…", icon: createElement(AgentBuddy, { agent, size: 13 }), run: () => openBuddyPicker(agent.id) }]
            : []),
        ...(resumeId
            ? [
                  pinned
                      ? { label: "Unpin", icon: menuIcon(IconPin), run: () => cmd.unpinChat(agent.type, resumeId) }
                      : {
                            label: "Pin",
                            icon: menuIcon(IconPin),
                            run: () => cmd.pinChat({ agent: agent.type, id: resumeId, title: agent.title, project: agent.cwd || session.cwd }),
                        },
              ]
            : []),
        ...(agent.launchState === "dormant"
            ? [{ label: "Resume", icon: menuIcon(IconRotate), run: () => cmd.selectAgent(agent.id) }]
            : agent.resumeId
              ? [{ label: "Sleep", icon: menuIcon(IconPower), run: () => cmd.sleepAgent(agent.id) }]
              : []),
        ...(agent.resumeId && agent.launchState !== "dormant"
            ? [
                  {
                      label: agent.keepAlive ? "Allow Auto-Sleep" : "Keep Alive",
                      icon: menuIcon(IconPin),
                      run: () => cmd.setAgentKeepAlive(agent.id, !agent.keepAlive),
                  },
              ]
            : []),
        {
            label: "Move to Project…",
            icon: menuIcon(IconFolder),
            disabled: !cmd.canMoveAgentToProject(getState(), agent),
            run: () => cmd.openMoveToProject(agent.id),
        },
        ...(agent.worktree
            ? [
                  { sep: true },
                  {
                      label: "Remove worktree…",
                      icon: menuIcon(IconTrash),
                      run: () => void import("../agents/agentWorktree").then(({ removeAgentWorktree }) => removeAgentWorktree(agent.id)),
                  },
              ]
            : []),
        { sep: true },
        { label: "Close", icon: menuIcon(IconClose), hint: hints.close, run: () => cmd.closeAgent(agent.id) },
        { label: "Close Others", icon: menuIcon(IconClose), disabled: others.length === 0, run: () => cmd.closeAgents(others.map((x) => x.id)) },
    ];
    if (canBypassPermissions(agent.type)) {
        const skip = isYolo(agent.permissionMode);
        items.push(
            { sep: true },
            {
                label: skip ? "Disable YOLO Mode" : "Enable YOLO Mode",
                icon: menuIcon(skip ? IconShield : IconShieldBolt),
                hint: hints.permissions,
                run: () => cmd.toggleAgentSkipPermissions(agent.id),
            },
        );
    }
    items.push({ sep: true });
    if (agentSupportsChat(agent.type)) items.push({ label: "Share Read-Only…", icon: menuIcon(IconShare), run: () => openShareDialog(agent.id) });
    items.push({
        label: "Copy Link",
        icon: menuIcon(IconCopy),
        disabled: !link,
        run: () => {
            if (link) void copyText(link).then(() => notify("success", "copied link"), reportError("copy"));
        },
    });
    return items;
}

export async function renameAgentPrompt(agent: Agent): Promise<void> {
    const title = await promptDialog({ title: "Edit title", label: "Title", initial: agent.title, confirmLabel: "Save" });
    if (title) cmd.renameAgent(agent.id, title);
}

export async function renameAgentNamePrompt(agent: Agent): Promise<void> {
    const name = await promptDialog({ title: "Rename agent", label: "Name", initial: agent.name ?? "", confirmLabel: "Rename" });
    if (name?.trim() && name.trim() !== agent.name) await cmd.changeAgentName(agent.id, name).catch(reportError("rename agent"));
}
