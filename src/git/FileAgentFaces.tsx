import { AgentFace } from "../buddies/Buddy";
import { isAgentType } from "@shared/agents/catalog";
import { useStore } from "../state/store";
import type { ChangeAuthor } from "../api/git";
import type { Agent } from "../state/types";

const MAX_FACES = 4;

/** The faces of the agents that wrote a file's changes, an author several agents could be standing for each of them. */
export function FileAgentFaces({ authors }: { authors: readonly ChangeAuthor[] }) {
    const agents = useStore((s) => s.agents);
    const faces = new Map<string, Pick<Agent, "id" | "type" | "buddy">>();
    for (const author of authors) {
        for (const id of author.shared?.length ? author.shared : [author.agentId]) {
            const agent = agents[id];
            const type = agent?.type ?? author.cli;
            if (!faces.has(id) && isAgentType(type)) faces.set(id, { id, type, buddy: agent?.buddy });
        }
    }
    if (faces.size === 0) return null;
    const names = authors.map((author) => author.name).join(", ");
    return (
        <span className="git-row-hint git-row-agents" title={`Written by ${names}`} aria-label={`Written by ${names}`}>
            {[...faces.values()].slice(0, MAX_FACES).map((face) => (
                <AgentFace
                    key={face.id}
                    agent={face}
                    size={14}
                    logoSize={16}
                    logoClass={`proj-pip proj-pip-${face.type}`}
                    buddyClass="proj-pip buddy-mark"
                />
            ))}
        </span>
    );
}
