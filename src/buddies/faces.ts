import type { AgentFace } from "../api/generated/wire";
import type { Agent } from "../state/types";
import { packById, resolveBuddy, type BuddyPack, type BuddyShape, type BuddyStyle } from "./catalogue";

export interface DrawnFace {
    pack: BuddyPack;
    shape: BuddyShape;
    color: string;
    /** The part of the shape's square its drawing fills, when the core said. */
    box?: string;
}

/**
 * The face the core gave the agent, drawn from this art; an agent the core has not listed yet, such as one just
 * started, wears the one the style gives it until it does.
 */
export function drawnFace(agent: Pick<Agent, "id" | "buddy">, face: AgentFace | null | undefined, style: BuddyStyle): DrawnFace {
    const pack = face ? packById(face.pack) : undefined;
    const shape = face && pack?.shapes.find((candidate) => candidate.id === face.shape);
    if (face && pack && shape) return { pack, shape, color: pack.colors[face.color % pack.colors.length], box: face.box };
    const look = resolveBuddy(agent, style);
    return { pack: look.pack, shape: look.shape, color: look.color };
}
