import type { ReactNode } from "react";
import { IS_MACOS, PRIMARY_SHORTCUT } from "../lib/platform";

export const CMD = PRIMARY_SHORTCUT;
export const ALT = IS_MACOS ? "⌥" : "Alt+";
export const SHIFT = IS_MACOS ? "⇧" : "Shift+";
export const CTRL = IS_MACOS ? "⌃" : "Ctrl+";

/** A styled shortcut chip for visible UI, e.g. <Kbd>{ALT}S</Kbd>. */
export function Kbd({ children }: { children: ReactNode }) {
    return <span className="kbd">{children}</span>;
}
