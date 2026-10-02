import { useShortcutLabel } from "../commands/useShortcutLabel";
import { TreeContextMenu } from "../rail/FileTree";
import { agentIdsOf } from "../state/selectors";
import { getState } from "../state/store";
import type { Agent, Session } from "../state/types";
import { agentMenu } from "./agentMenu";

export function AgentContextMenu({
    agent,
    session,
    x,
    y,
    onClose,
    onRename,
}: {
    agent: Agent;
    session: Session;
    x: number;
    y: number;
    onClose: () => void;
    onRename: () => void;
}) {
    const close = useShortcutLabel("pane.close");
    const permissions = useShortcutLabel("agent.permissions");
    const state = getState();
    const others = agentIdsOf(state, session.id)
        .map((id) => state.agents[id])
        .filter((other): other is Agent => !!other && other.id !== agent.id);
    return <TreeContextMenu x={x} y={y} items={agentMenu(agent, others, session, { close, permissions }, onRename)} onClose={onClose} />;
}
