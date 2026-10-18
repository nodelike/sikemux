import type { ReactNode } from "react";
import { useShortcutLabel } from "../commands/useShortcutLabel";
import type { KeybindingActionId } from "../commands/keybindings";
import { EmptyState, type EmptyStateAction } from "../ui/Panel";

/** A rail page with nothing to list: a faint mark, what is missing, and the key that fills it. */
export function RailEmpty({
    icon,
    title,
    message,
    shortcut,
    doing,
    action,
}: {
    icon: ReactNode;
    title: string;
    message?: string;
    shortcut?: KeybindingActionId;
    doing?: string;
    action?: EmptyStateAction;
}) {
    const label = useShortcutLabel(shortcut ?? "settings.toggle");
    const press =
        shortcut && label && doing ? (
            <>
                Press <kbd>{label}</kbd> to {doing}
            </>
        ) : null;
    return <EmptyState icon={icon} title={title} message={press ?? message ?? ""} action={action} />;
}
