import { codeFence } from "../agents/agentTargets";
import type { AgentDelivery } from "../agents/agentInbox";
import { collectPanes } from "../state/layout";
import type { StoreState } from "../state/store";
import type { PtyContext } from "../state/types";

export function terminalName(state: Pick<StoreState, "windows">, context: PtyContext | undefined): string | null {
    const win = context?.windowId ? state.windows[context.windowId] : undefined;
    if (!win) return null;
    const pane = context?.paneId ? collectPanes(win.root).find((candidate) => candidate.id === context.paneId) : undefined;
    return pane?.title || win.name || null;
}

export function terminalSelectionDelivery(selection: string, name: string | null, cwd: string | null | undefined): AgentDelivery {
    const where = cwd ? ` in ${cwd}` : "";
    const heading = name ? `From the terminal "${name}"${where}:` : `From a terminal${where}:`;
    return { text: `${heading}\n\n${codeFence(selection)}\n` };
}
