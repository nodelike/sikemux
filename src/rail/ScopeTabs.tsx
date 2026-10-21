import type { ReactNode } from "react";
import { hostTitle, type HostChoice } from "../hosts/hosts";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import type { TabRailScope } from "../state/types";
import { IconFolder, IconLaptop, IconServer, IconStack } from "../ui/Icons";

/** The project or host chosen in the side rail, by name, or every open project, as one track the choice slides along. */
export function ScopeTabs({
    scope,
    name,
    icon = <IconFolder size={13} />,
    waitingElsewhere = 0,
    onChange,
}: {
    scope: TabRailScope;
    name: string;
    icon?: ReactNode;
    waitingElsewhere?: number;
    onChange: (scope: TabRailScope) => void;
}) {
    return (
        <div className={`scope-track${scope === "all" ? " is-all" : ""}`} role="tablist" aria-label="Which projects">
            <span className="scope-thumb" aria-hidden="true" />
            <button type="button" role="tab" aria-selected={scope === "project"} aria-label={name} onClick={() => onChange("project")}>
                {icon}
                <span className="scope-name">{name}</span>
            </button>
            <button
                type="button"
                role="tab"
                aria-selected={scope === "all"}
                aria-label={waitingElsewhere > 0 ? `All projects, ${waitingElsewhere} waiting` : "All projects"}
                onClick={() => onChange("all")}>
                <IconStack size={13} />
                All projects
                {waitingElsewhere > 0 && <span className="scope-count">{waitingElsewhere}</span>}
            </button>
        </div>
    );
}

/** The scope tabs with the chosen host's name in place of a project's. */
export function HostScopeTabs({ host }: { host: HostChoice }) {
    const scope = useStore((s) => s.tabRailScope);
    const name = useStore((s) => hostTitle(s, host));
    const icon = host.kind === "core" && !host.host ? <IconLaptop size={13} /> : <IconServer size={13} />;
    return <ScopeTabs scope={scope} name={name} icon={icon} onChange={cmd.setTabRailScope} />;
}
