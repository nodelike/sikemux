import type { ReactNode } from "react";
import type { KeybindingActionId } from "../commands/keybindings";
import { useShortcutLabel } from "../commands/useShortcutLabel";

export function NewRow({ icon, title, shortcut, onClick }: { icon: ReactNode; title: string; shortcut: KeybindingActionId; onClick: () => void }) {
    const label = useShortcutLabel(shortcut);
    return (
        <button type="button" className="agent-row agent-new" onClick={onClick}>
            <span className="agent-glyph">{icon}</span>
            <span className="agent-title">{title}</span>
            {label && <span className="agent-new-kbd">{label}</span>}
        </button>
    );
}
