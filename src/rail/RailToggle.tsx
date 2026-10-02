import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { IconPanelLeft, IconPanelRight } from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";

const RAILS = {
    start: { name: "sessions rail", Icon: IconPanelLeft, toggle: cmd.toggleSideRail },
    end: { name: "agents rail", Icon: IconPanelRight, toggle: cmd.toggleAgentRail },
} as const;

export function RailToggle({ edge }: { edge: "start" | "end" }) {
    const open = useStore((s) => (edge === "start" ? s.sideRailOpen : s.agentRailOpen));
    const { name, Icon, toggle } = RAILS[edge];
    const label = open ? `Hide ${name}` : `Keep ${name} open`;
    return (
        <Tooltip label={label}>
            <button className="agent-header-action rail-toggle" onClick={toggle} aria-expanded={open} aria-label={label}>
                <Icon size={15} />
            </button>
        </Tooltip>
    );
}
