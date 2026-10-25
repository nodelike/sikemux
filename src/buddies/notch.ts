import type { AgentFace } from "@shared/wire.generated";
import { packById } from "@shared/buddies/catalogue";
import { buddyMarkup } from "@shared/buddies/render";
import type { BuddyState } from "@shared/buddies/shared";

/* Some shapes repeat an attribute on one tag. A page keeps the first; the notch's SVG reader rejects the whole drawing, so the repeats go. */
function firstAttributes(markup: string): string {
    return markup.replace(/<[a-zA-Z][^>]*>/g, (tag) => {
        const seen = new Set<string>();
        return tag.replace(/\s([a-zA-Z:-]+)="[^"]*"/g, (attribute, name: string) => (seen.has(name) ? "" : (seen.add(name), attribute)));
    });
}

/** The face the core gave an agent, as a standalone SVG document for the notch to draw; null for one it does not know. */
export function notchFace(face: AgentFace, state: BuddyState): string | null {
    const pack = packById(face.pack);
    const shape = pack?.shapes.find((candidate) => candidate.id === face.shape);
    if (!pack || !shape) return null;
    const color = pack.colors[face.color % pack.colors.length];
    const markup = buddyMarkup(pack, shape, color, state, "notch", face.box).replace("<svg ", '<svg xmlns="http://www.w3.org/2000/svg" ');
    return firstAttributes(markup);
}
