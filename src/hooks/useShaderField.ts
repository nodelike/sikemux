import { useEffect, useRef } from "react";
import { mountShaderField, unmountShaderField, type ShaderFieldPreset } from "../lib/shaderField";

/**
 * Attach a Paper Shaders field to an element for as long as it is mounted.
 *
 * The returned ref goes on an empty, `aria-hidden` element that the shader
 * paints into — never on one with content, since the runtime puts its canvas
 * behind the host's own children and sizes it to the host.
 *
 * Nothing here reports success. A field is decoration: when the budget is spent
 * or WebGL is missing the hook is a no-op and the element stays empty, which is
 * why every surface is styled to look deliberate with no canvas in it.
 */
export function useShaderField<T extends HTMLElement>(preset: ShaderFieldPreset, enabled = true) {
    const ref = useRef<T | null>(null);
    useEffect(() => {
        const host = ref.current;
        if (!host || !enabled) return;
        mountShaderField(host, preset);
        return () => unmountShaderField(host);
    }, [preset, enabled]);
    return ref;
}
