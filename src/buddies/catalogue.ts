import { chibiPack, japan } from "./packs/characters";
import { buddies, personas } from "./packs/blobs";
import { cyclops, glow, googly, matrix, peekers, soot } from "./packs/eyes";
import { screens } from "./packs/screens";
import type { BuddyPack, BuddyShape, BuddyState } from "./shared";
import type { Agent } from "../state/types";
import type { AgentGroup } from "../state/agentStatus";
import { EVERY_PACK, EVERY_SHAPE, type BuddyStyle } from "./style";

export { DEFAULT_BUDDY_STYLE, EVERY_PACK, EVERY_SHAPE, type BuddyStyle } from "./style";

export type { BuddyPack, BuddyShape, BuddyState } from "./shared";

export const BUDDY_PACKS: readonly BuddyPack[] = [buddies, glow, googly, soot, peekers, cyclops, matrix, screens, japan, chibiPack, personas];

export interface ResolvedBuddy {
    pack: BuddyPack;
    shape: BuddyShape;
    color: string;
    colorIndex: number;
}

export const packById = (id: string): BuddyPack | undefined => BUDDY_PACKS.find((pack) => pack.id === id);

function hash(text: string): number {
    let h = 2166136261;
    for (let i = 0; i < text.length; i++) {
        h ^= text.charCodeAt(i);
        h = Math.imul(h, 16777619);
    }
    h ^= h >>> 16;
    h = Math.imul(h, 0x85ebca6b);
    h ^= h >>> 13;
    return h >>> 0;
}

export function resolveBuddy(agent: Pick<Agent, "id" | "buddy">, style: BuddyStyle): ResolvedBuddy {
    const own = agent.buddy;
    const h = hash(agent.id);
    let pack = own?.pack ? packById(own.pack) : undefined;
    let shape = own?.shape ? pack?.shapes.find((candidate) => candidate.id === own.shape) : undefined;
    if (!pack || !shape) {
        const everyPack = style.pack === EVERY_PACK;
        pack = everyPack ? BUDDY_PACKS[h % BUDDY_PACKS.length] : (packById(style.pack) ?? BUDDY_PACKS[0]);
        shape =
            everyPack || style.shape === EVERY_SHAPE
                ? pack.shapes[(h >>> 8) % pack.shapes.length]
                : (pack.shapes.find((candidate) => candidate.id === style.shape) ?? pack.shapes[0]);
    }
    const colorIndex = (own?.color ?? h) % pack.colors.length;
    return { pack, shape, color: pack.colors[colorIndex], colorIndex };
}

export function buddyStateOfGroup(group: AgentGroup | undefined): BuddyState {
    switch (group) {
        case "blocked":
            return "waiting";
        case "working":
            return "working";
        case "done":
            return "done";
        default:
            return "idle";
    }
}
