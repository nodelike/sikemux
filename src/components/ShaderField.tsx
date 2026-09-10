import { useShaderField } from "../hooks/useShaderField";
import type { ShaderFieldPreset } from "../lib/shaderField";

/**
 * An empty element for a Paper Shaders field to paint into.
 *
 * Decoration only, and never a layout participant — every surface is styled to
 * look deliberate with no canvas in it, because the budget may be spent or the
 * machine may have no WebGL.
 */
export function ShaderField({ preset, className, enabled = true }: { preset: ShaderFieldPreset; className: string; enabled?: boolean }) {
    const ref = useShaderField<HTMLDivElement>(preset, enabled);
    return <div className={className} aria-hidden="true" ref={ref} />;
}
