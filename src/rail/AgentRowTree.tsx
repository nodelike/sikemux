import { IconFolder } from "@shared/ui/Icons";

/** The project a row's agent works in, on a second line under its title when the rail shows every project. */
export function AgentRowProject({ name }: { name: string }) {
    return (
        <span className="agent-row-project">
            <IconFolder size={11} />
            <span>{name}</span>
        </span>
    );
}
