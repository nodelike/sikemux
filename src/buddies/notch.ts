import { publishedBuddy, wearsBuddy, type PublishedBuddies } from "./published";
import { buddyMarkup } from "./render";
import type { BuddyState } from "./shared";

/* Some shapes repeat an attribute on one tag. A page keeps the first; the notch's SVG reader rejects the whole drawing, so the repeats go. */
function firstAttributes(markup: string): string {
    return markup.replace(/<[a-zA-Z][^>]*>/g, (tag) => {
        const seen = new Set<string>();
        return tag.replace(/\s([a-zA-Z:-]+)="[^"]*"/g, (attribute, name: string) => (seen.has(name) ? "" : (seen.add(name), attribute)));
    });
}

/** An agent's buddy as a standalone SVG document, for the notch to draw without a page; null when it shows its agent's logo instead. */
export function notchBuddy(buddies: PublishedBuddies, agentId: string, named: boolean, state: BuddyState): string | null {
    if (!wearsBuddy(buddies.on, named)) return null;
    const { pack, shape, color, box } = publishedBuddy(buddies, agentId);
    const markup = buddyMarkup(pack, shape, color, state, "notch", box).replace("<svg ", '<svg xmlns="http://www.w3.org/2000/svg" ');
    return firstAttributes(markup);
}
