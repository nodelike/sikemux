import { createContext, useContext, useEffect, useRef, useState, type RefObject } from "react";
import { usePaneImageState } from "../lib/paneImage";
import { useShaderField } from "../hooks/useShaderField";
import { reviveShaderField, type ShaderFieldPreset } from "../lib/shaderField";
import { whenStageStill } from "../state/nativeViews";
import { useStore } from "../state/store";
import { preferencesOf } from "../state/preferences";

/**
 * An empty element for a Paper Shaders field to paint into.
 *
 * Decoration only, and never a layout participant — every surface is styled to
 * look deliberate with no canvas in it, because the budget may be spent or the
 * machine may have no WebGL.
 */
export function ShaderField({
    preset,
    className,
    enabled = true,
    image = null,
    hostRef,
}: {
    preset: ShaderFieldPreset;
    className: string;
    enabled?: boolean;
    image?: HTMLImageElement | null;
    hostRef?: RefObject<HTMLDivElement | null>;
}) {
    const ref = useShaderField<HTMLDivElement>(preset, enabled, image);
    return (
        <div
            className={className}
            aria-hidden="true"
            ref={(element) => {
                ref.current = element;
                if (hostRef) hostRef.current = element;
            }}
        />
    );
}

/** Whether the pane around draws no picture of its own and leaves one to a screen inside it, as a plugin's pane does for its sign-in. */
export const ScreenFieldContext = createContext(false);

/** The pane's picture behind one screen of a pane that draws none of its own. */
export function ScreenField() {
    const wanted = useContext(ScreenFieldContext);
    const paneShader = useStore((s) => preferencesOf(s).paneShader);
    return wanted ? <PaneField enabled={paneShader} /> : null;
}

/** Whether the screen a pane sits on is on the stage, standing still or sliding. */
export const PanePaintedContext = createContext(true);

/**
 * The grain behind a pane, or the reader's picture in its place when one is set.
 *
 * It starts the first time its screen comes on stage and is kept from then on:
 * starting one stalls the window for a good part of a swipe, so a screen swiped
 * back and forth would otherwise pay that on every swipe.
 */
export function PaneField({ enabled }: { enabled: boolean }) {
    const { image, loading } = usePaneImageState();
    const painted = useContext(PanePaintedContext);
    // Starting a field stalls a frame, so a screen's first slide onto the stage goes without it until the slide lands.
    const [shown, setShown] = useState(false);
    useEffect(() => {
        if (!painted || shown) return;
        return whenStageStill(() => setShown(true));
    }, [painted, shown]);
    const field = image ? "image" : "ambient";
    const host = useRef<HTMLDivElement | null>(null);
    useEffect(() => {
        if (painted && host.current) reviveShaderField(host.current);
    }, [painted]);
    return (
        <ShaderField
            hostRef={host}
            preset={field}
            className={`pane-field pane-field-${field}`}
            enabled={enabled && shown && !loading}
            image={image}
        />
    );
}
