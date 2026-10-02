import { useLayoutEffect, useState } from "react";
import { createPortal } from "react-dom";
import { AgentPalette } from "./AgentPalette";

export function AgentPalettePortal() {
    const [host, setHost] = useState<HTMLElement | null>(null);

    useLayoutEffect(() => {
        setHost(document.querySelector<HTMLElement>(".stage"));
    }, []);

    if (!host) return null;
    return createPortal(
        <div className="agent-palette-stage">
            <AgentPalette />
        </div>,
        host,
    );
}
