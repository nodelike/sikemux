import type { ReactNode } from "react";

/** A styled shortcut chip for visible UI, e.g. <Kbd>⌘K</Kbd>. */
export function Kbd({ children }: { children: ReactNode }) {
    return <span className="kbd">{children}</span>;
}
