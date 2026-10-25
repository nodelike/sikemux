import { createPortal } from "react-dom";
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { IS_MACOS } from "@shared/lib/platform";
import { IconCopy, IconDiscard, IconMessage, IconSearch, IconText } from "../ui/Icons";
import { useOccludeNativeViews } from "../state/nativeViews";
import type { TerminalController } from "./useTerminal";

interface MenuItem {
    label?: string;
    hint?: string;
    icon?: ReactNode;
    disabled?: boolean;
    separator?: boolean;
    run?: () => void | Promise<unknown>;
}

export function TerminalContextMenu({
    x,
    y,
    controller,
    onFind,
    onSendSelection,
    onClose,
}: {
    x: number;
    y: number;
    controller: TerminalController;
    onFind: (seed: string) => void;
    /** Left out where there is no project whose agents could take it. */
    onSendSelection?: (selection: string) => void;
    onClose: () => void;
}) {
    useOccludeNativeViews(true);
    const ref = useRef<HTMLDivElement>(null);
    const [pos, setPos] = useState({ left: x, top: y });
    const selection = controller.getSelection();

    useLayoutEffect(() => {
        const rect = ref.current?.getBoundingClientRect();
        if (!rect) return;
        const pad = 6;
        setPos({
            left: Math.max(pad, Math.min(x, window.innerWidth - rect.width - pad)),
            top: Math.max(pad, Math.min(y, window.innerHeight - rect.height - pad)),
        });
    }, [x, y]);

    useEffect(() => {
        const onKeyDown = (event: KeyboardEvent) => {
            if (event.key === "Escape") onClose();
        };
        window.addEventListener("keydown", onKeyDown, true);
        return () => window.removeEventListener("keydown", onKeyDown, true);
    }, [onClose]);

    const run = (action: () => void | Promise<unknown>) => {
        onClose();
        try {
            void Promise.resolve(action()).catch((error) => console.warn("terminal action failed", error));
        } catch (error) {
            console.warn("terminal action failed", error);
        }
    };
    const primary = IS_MACOS ? "⌘" : "Ctrl+";
    const items: MenuItem[] = [
        { label: "Copy", hint: `${primary}C`, icon: <IconCopy size={13} />, disabled: !selection, run: () => controller.copySelection() },
        { label: "Paste", hint: `${primary}V`, icon: <IconText size={13} />, run: () => controller.pasteClipboard() },
        { label: "Select All", hint: `${primary}A`, icon: <IconText size={13} />, run: () => controller.selectAll() },
        ...(selection && onSendSelection
            ? [{ label: "Send Selection to Agent…", icon: <IconMessage size={13} />, run: () => onSendSelection(selection) }]
            : []),
        { separator: true },
        { label: "Find", hint: `${primary}F`, icon: <IconSearch size={13} />, run: () => onFind(selection) },
        { label: "Copy Scrollback", icon: <IconCopy size={13} />, run: () => controller.copyScrollback() },
        { separator: true },
        { label: "Clear Terminal", icon: <IconDiscard size={13} />, run: () => controller.clear() },
    ];

    return createPortal(
        <div
            className="terminal-menu-scrim"
            onMouseDown={onClose}
            onContextMenu={(event) => {
                event.preventDefault();
                onClose();
            }}>
            <div ref={ref} className="terminal-menu" style={pos} role="menu" onMouseDown={(event) => event.stopPropagation()}>
                {items.map((item, index) =>
                    item.separator ? (
                        <div className="terminal-menu-separator" key={index} />
                    ) : (
                        <button type="button" role="menuitem" key={item.label} disabled={item.disabled} onClick={() => item.run && run(item.run)}>
                            <span className="tree-ctx-icon">{item.icon}</span>
                            <span>{item.label}</span>
                            {item.hint && <kbd>{item.hint}</kbd>}
                        </button>
                    ),
                )}
            </div>
        </div>,
        document.body,
    );
}
