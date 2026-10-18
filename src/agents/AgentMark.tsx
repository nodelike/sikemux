import { AgentIcon } from "../ui/Icons";
import { isAgentType } from "./catalog";

/** The mark of the CLI an agent runs, or nothing for one Sikemux does not know. */
export function AgentMark({ cli, size = 14 }: { cli: string | null | undefined; size?: number }) {
    return cli && isAgentType(cli) ? <AgentIcon type={cli} size={size} className={`agent-glyph ${cli}`} /> : null;
}
