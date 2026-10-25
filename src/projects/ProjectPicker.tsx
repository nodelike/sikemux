import { useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { projectMark } from "../hosts/placeMarks";
import { useResourceEnabled } from "../state/resources";
import { projectRootsScanR } from "../state/resources.defs";
import { projectRootsOf } from "../state/preferences";
import { useStore } from "../state/store";
import { Dropdown, type DropdownOption } from "../ui/Dropdown";
import { IconFolder } from "../ui/Icons";
import { dirname } from "@shared/lib/paths";
import { projectChoices, type ProjectChoice } from "./projectChoices";

/** The projects open on a host, then on this host the folders found under the project roots. */
export function ProjectPicker({
    host,
    value,
    onPick,
    className,
    title,
    opens,
}: {
    host: string | undefined;
    /** The chosen project's folder. */
    value: string;
    onPick: (choice: ProjectChoice) => void;
    className?: string;
    title?: string;
    opens?: "above" | "below";
}) {
    const home = useStore((s) => s.home);
    const roots = useStore(projectRootsOf);
    const sessions = useStore(
        useShallow((s) => s.sessionOrder.map((id) => s.sessions[id]).filter((session) => session?.kind === "project" && session.host === host)),
    );
    const [open, setOpen] = useState(false);
    const found = useResourceEnabled(open && host === undefined && roots.length > 0, projectRootsScanR, roots).data;

    const choices = useMemo(() => projectChoices(sessions, found ?? [], roots, home), [sessions, found, roots, home]);
    const options = useMemo<DropdownOption[]>(
        () =>
            choices.map((choice) => ({
                value: choice.path,
                label: choice.name,
                detail: dirname(choice.sub),
                className: "inline-detail",
                icon: choice.sessionId ? projectMark() : <IconFolder size={13} />,
                group: choice.sessionId ? "Open" : "Found on disk",
            })),
        [choices],
    );

    return (
        <Dropdown
            className={className}
            label="Project"
            title={title}
            value={value}
            options={options}
            icon={projectMark()}
            menuWidth={300}
            opens={opens}
            search="Find a project"
            checkAtEnd
            onOpenChange={setOpen}
            onChange={(path) => {
                const choice = choices.find((candidate) => candidate.path === path);
                if (choice) onPick(choice);
            }}
        />
    );
}
