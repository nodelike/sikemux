/** A style whose pack is this mixes every pack, one per agent. */
export const EVERY_PACK = "all";
/** A style whose shape is this gives each agent its own shape from the pack. */
export const EVERY_SHAPE = "mixed";

export interface BuddyStyle {
    pack: string;
    shape: string;
}

export const DEFAULT_BUDDY_STYLE: BuddyStyle = { pack: EVERY_PACK, shape: EVERY_SHAPE };
