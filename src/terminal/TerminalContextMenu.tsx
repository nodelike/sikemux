import { createPortal } from "react-dom";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { IS_MACOS } from "../lib/platform";
import type { TerminalController } from "./useXterm";

interface MenuItem {
    label?: string;
    hint?: string;
    disabled?: boolean;
    separator?: boolean;
    run?: () => void | Promise<unknown>;
}

export function TerminalContextMenu({
    x,
    y,
    controller,
    onFind,
    onClose,
}: {
    x: number;
    y: number;
    controller: TerminalController;
    onFind: (seed: string) => void;
    onClose: () => void;
}) {
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
        { label: "Copy", hint: `${primary}C`, disabled: !selection, run: () => controller.copySelection() },
        { label: "Paste", hint: `${primary}V`, run: () => controller.pasteClipboard() },
        { label: "Select All", hint: `${primary}A`, run: () => controller.selectAll() },
        { separator: true },
        { label: "Find", hint: `${primary}F`, run: () => onFind(selection) },
        { label: "Copy Scrollback", run: () => controller.copyScrollback() },
        { separator: true },
        { label: "Clear Terminal", run: () => controller.clear() },
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
