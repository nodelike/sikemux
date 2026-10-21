import { EVERY_PACK, packById, resolveBuddy, type BuddyPack, type BuddyShape, type BuddyStyle } from "./catalogue";
import { buddyBox, FULL_BOX } from "./render";
import type { Agent } from "../state/types";
import type { BuddyLook, PublishedBuddies } from "../api/generated/wire";

export type { PublishedBuddies };

export interface DrawnBuddy {
    pack: BuddyPack;
    shape: BuddyShape;
    color: string;
    box: string;
}

const boxKey = (pack: BuddyPack, shape: BuddyShape) => `${pack.id}/${shape.id}`;

/** Whether an agent shows its buddy rather than its agent's logo: always once the person named it, otherwise while buddies are on. */
export const wearsBuddy = (on: boolean, named: boolean): boolean => on || named;

/** Every agent's buddy, with the boxes of the shapes they wear and of every shape in the chosen pack, for buddies a phone resolves itself. */
export function publishBuddies(agents: Iterable<Pick<Agent, "id" | "buddy">>, style: BuddyStyle, motion: boolean, on: boolean): PublishedBuddies {
    const looks: Record<string, BuddyLook> = {};
    const boxes: Record<string, string> = {};
    const measure = (pack: BuddyPack, shape: BuddyShape) => (boxes[boxKey(pack, shape)] = buddyBox(pack, shape));
    for (const agent of agents) {
        const { pack, shape, color } = resolveBuddy(agent, style);
        looks[agent.id] = { pack: pack.id, shape: shape.id, color };
        measure(pack, shape);
    }
    const chosen = style.pack === EVERY_PACK ? undefined : packById(style.pack);
    if (chosen) for (const shape of chosen.shapes) measure(chosen, shape);
    return { on, pack: style.pack, shape: style.shape, motion, agents: looks, boxes };
}

/** The buddy the app published for an agent, or the one the published style gives it until the app has. */
export function publishedBuddy(buddies: PublishedBuddies, agentId: string): DrawnBuddy {
    const look = buddies.agents[agentId];
    const pack = look && packById(look.pack);
    const shape = look && pack?.shapes.find((candidate) => candidate.id === look.shape);
    const drawn =
        look && pack && shape ? { pack, shape, color: look.color } : resolveBuddy({ id: agentId }, { pack: buddies.pack, shape: buddies.shape });
    return { ...drawn, box: buddies.boxes[boxKey(drawn.pack, drawn.shape)] ?? FULL_BOX };
}
