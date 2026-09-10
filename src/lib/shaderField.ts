/*
 * Paper Shaders, used only where the workspace has nothing to show yet.
 *
 * The scarce resource here is WebGL contexts, not frames. Every open terminal
 * loads xterm's WebGL renderer (see `src/terminal/useXterm.ts`), and a page is
 * capped at roughly sixteen contexts before the browser starts evicting the
 * oldest without warning. Losing a decoration costs nothing; losing a
 * terminal's renderer costs the product. So the budget below is deliberately
 * small, the runtime is fetched on first use, and a surface is released the
 * moment its host leaves the document.
 *
 * Every mount is best effort. No WebGL, a failed texture decode, a reader who
 * asked for less motion — the plain interface underneath is always the
 * fallback, never a blank hole.
 *
 * Colours come from the theme bus rather than from computed custom properties,
 * so a shader is tinted from the same typed values the editor and terminal read
 * and re-tints in place when the theme changes.
 */

// Type-only, so the runtime itself stays behind the dynamic import below.
import type { ShaderMountUniforms } from "@paper-design/shaders";
import type { Theme } from "../themes";
import { currentTheme, subscribeTheme } from "../themes/bus";

type Shaders = typeof import("@paper-design/shaders");

/*
 * Only the members two presets actually touch.
 *
 * This narrowness is load-bearing, not tidiness: the package's entry point
 * re-exports all twenty-eight shaders, and each fragment shader is a large
 * string literal. Holding the whole module namespace and reaching into it
 * dynamically leaves Rollup no choice but to keep every one of them — it cost
 * 212 kB when this was written that way. Naming the members means the import is
 * destructured statically and the other twenty-six shake out.
 */
interface Runtime {
    ShaderMount: Shaders["ShaderMount"];
    ditheringFragmentShader: string;
    DitheringShapes: Shaders["DitheringShapes"];
    DitheringTypes: Shaders["DitheringTypes"];
    grainGradientFragmentShader: string;
    GrainGradientShapes: Shaders["GrainGradientShapes"];
    ShaderFitOptions: Shaders["ShaderFitOptions"];
    getShaderColorFromString: Shaders["getShaderColorFromString"];
    getShaderNoiseTexture: Shaders["getShaderNoiseTexture"];
}

export type ShaderFieldPreset = "empty" | "onboarding";

/*
 * Two surfaces. The editor's empty state and the first-run tour are never on
 * screen at the same time as each other in practice, so this is really "one,
 * plus room for a handover" — and it leaves the rest of the context budget to
 * the terminals that need it.
 */
const SURFACE_BUDGET = 2;

interface Surface {
    preset: ShaderFieldPreset;
    mount: InstanceType<Shaders["ShaderMount"]> | null;
    runtime: Runtime | null;
}

const surfaces = new Map<HTMLElement, Surface>();
let runtimePromise: Promise<Runtime | null> | null = null;
let noisePromise: Promise<HTMLImageElement> | null = null;
let webglSupported: boolean | null = null;

/*
 * Probing costs a context, so the result is remembered and the throwaway one is
 * handed straight back. jsdom has no WebGL at all, which is why this exists:
 * without it every test that renders an empty pane would queue a mount that can
 * only fail.
 */
function webglAvailable(): boolean {
    if (webglSupported !== null) return webglSupported;
    try {
        const gl = document.createElement("canvas").getContext("webgl2");
        webglSupported = Boolean(gl);
        gl?.getExtension("WEBGL_lose_context")?.loseContext();
    } catch {
        webglSupported = false;
    }
    return webglSupported;
}

// Motion is the decoration; the field itself is not. Asked for less of it, the
// shaders render one static frame rather than disappearing.
function shouldAnimate(): boolean {
    return !window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
}

function loadRuntime(): Promise<Runtime | null> {
    runtimePromise ??= import("@paper-design/shaders")
        .then(
            ({
                ShaderMount,
                ditheringFragmentShader,
                DitheringShapes,
                DitheringTypes,
                grainGradientFragmentShader,
                GrainGradientShapes,
                ShaderFitOptions,
                getShaderColorFromString,
                getShaderNoiseTexture,
            }): Runtime => ({
                ShaderMount,
                ditheringFragmentShader,
                DitheringShapes,
                DitheringTypes,
                grainGradientFragmentShader,
                GrainGradientShapes,
                ShaderFitOptions,
                getShaderColorFromString,
                getShaderNoiseTexture,
            }),
        )
        .catch((error: unknown) => {
            console.warn("Paper Shaders unavailable:", error instanceof Error ? error.message : error);
            return null;
        });
    return runtimePromise;
}

// The grain gradient samples a noise texture, which has to be decoded before it
// can be handed to WebGL.
function loadNoise(runtime: Runtime): Promise<HTMLImageElement> {
    noisePromise ??= new Promise((resolve, reject) => {
        const image = runtime.getShaderNoiseTexture();
        if (!image) {
            reject(new Error("noise texture is unavailable"));
            return;
        }
        if (image.complete && image.naturalWidth) {
            resolve(image);
            return;
        }
        image.addEventListener("load", () => resolve(image), { once: true });
        image.addEventListener("error", () => reject(new Error("noise texture failed to decode")), { once: true });
    });
    return noisePromise;
}

function sizing(runtime: Runtime, fit: "none" | "contain" | "cover", scale: number) {
    return {
        u_fit: runtime.ShaderFitOptions[fit],
        u_scale: scale,
        u_rotation: 0,
        u_offsetX: 0,
        u_offsetY: 0,
        u_originX: 0.5,
        u_originY: 0.5,
        u_worldWidth: 0,
        u_worldHeight: 0,
    };
}

interface Recipe {
    fragmentShader: string;
    speed: number;
    uniforms: ShaderMountUniforms;
    needsNoise?: boolean;
}

const PRESETS: Record<ShaderFieldPreset, (runtime: Runtime, theme: Theme) => Recipe> = {
    /*
     * A pane with no file in it. Two-colour dithering drifts under the copy as
     * an accent-tinted cloud: it reads as depth in the corner of the eye and
     * never as an animation asking to be watched. Masked to a disc in CSS so it
     * has no edges of its own to compete with the text.
     */
    empty: (runtime, theme) => ({
        fragmentShader: runtime.ditheringFragmentShader,
        speed: 0.22,
        uniforms: {
            u_colorBack: runtime.getShaderColorFromString(theme.chrome.bgDim),
            u_colorFront: runtime.getShaderColorFromString(theme.chrome.acc),
            u_shape: runtime.DitheringShapes.simplex,
            u_type: runtime.DitheringTypes["4x4"],
            u_pxSize: 2,
            ...sizing(runtime, "none", 0.5),
        },
    }),

    /*
     * The first-run tour. The one place in the app with nothing to do and no
     * data on screen, so it gets the richer shader at real strength.
     *
     * The four hues are syntax-highlighting colours rather than chrome ones,
     * which is what makes this themeable at all: chrome is a single accent plus
     * greys, so a four-colour gradient built from it collapses into one hue,
     * while keyword/string/type/tag are perceptually distinct in any theme
     * worth shipping.
     */
    onboarding: (runtime, theme) => ({
        fragmentShader: runtime.grainGradientFragmentShader,
        speed: 0.16,
        uniforms: {
            u_colorBack: runtime.getShaderColorFromString(theme.chrome.bgDim),
            u_colors: [theme.highlight.keyword, theme.highlight.string, theme.highlight.type, theme.highlight.tag].map((hue) =>
                runtime.getShaderColorFromString(hue),
            ),
            u_colorsCount: 4,
            u_softness: 0.85,
            u_intensity: 0.3,
            u_noise: 0.5,
            u_shape: runtime.GrainGradientShapes.corners,
            ...sizing(runtime, "cover", 1.1),
        },
        needsNoise: true,
    }),
};

/*
 * A live surface whose host has been torn out of the tree takes its context
 * with it and nothing tells us, so orphans are released whenever another mount
 * is asked for.
 */
function prune(): void {
    for (const host of [...surfaces.keys()]) {
        if (!host.isConnected) unmountShaderField(host);
    }
}

/**
 * Mount a shader behind the contents of `host`.
 *
 * Returns nothing useful on purpose: nothing in the interface may depend on a
 * surface existing, so every caller has to still work when this quietly does
 * nothing.
 */
export function mountShaderField(host: HTMLElement, preset: ShaderFieldPreset): void {
    if (!webglAvailable() || surfaces.has(host)) return;
    prune();
    if (surfaces.size >= SURFACE_BUDGET) return;

    // Claimed before the first await so a burst of calls mounts once.
    surfaces.set(host, { preset, mount: null, runtime: null });
    void (async () => {
        const runtime = await loadRuntime();
        if (!runtime || surfaces.get(host)?.mount !== null) {
            if (!runtime) surfaces.delete(host);
            return;
        }
        try {
            const recipe = PRESETS[preset](runtime, currentTheme());
            if (recipe.needsNoise) recipe.uniforms.u_noiseTexture = await loadNoise(runtime);
            // Unmounted while the runtime or the texture was loading.
            if (!surfaces.has(host) || !host.isConnected) {
                surfaces.delete(host);
                return;
            }
            const animate = shouldAnimate();
            const mount = new runtime.ShaderMount(
                host,
                recipe.fragmentShader,
                recipe.uniforms,
                { antialias: false },
                animate ? recipe.speed : 0,
                animate ? 0 : 2500,
                // These are soft fields behind text, not artwork. Rendering at
                // 1x rather than the default 2x halves the fill cost and is
                // invisible through the mask.
                1,
            );
            surfaces.set(host, { preset, mount, runtime });
            host.dataset.shaderField = preset;
        } catch (error) {
            console.warn(`Paper Shaders: ${preset} field skipped —`, error instanceof Error ? error.message : error);
            surfaces.delete(host);
        }
    })();
}

/** Release a surface and its WebGL context. Safe to call for a host that never got one. */
export function unmountShaderField(host: HTMLElement): void {
    const surface = surfaces.get(host);
    if (!surface) return;
    surfaces.delete(host);
    delete host.dataset.shaderField;
    surface.mount?.dispose();
}

/** Live surface count. Exported for tests and for reasoning about the context budget. */
export function shaderFieldCount(): number {
    return surfaces.size;
}

/*
 * Re-tint in place on a theme change. Remounting would drop and reacquire a
 * context for what is only a handful of uniforms, and the theme bus already
 * fires for the editor and terminal at the same moment.
 */
subscribeTheme((theme) => {
    for (const [host, surface] of surfaces) {
        if (!surface.mount || !surface.runtime) continue;
        try {
            surface.mount.setUniforms(PRESETS[surface.preset](surface.runtime, theme).uniforms);
        } catch (error) {
            console.warn(`Paper Shaders: ${surface.preset} re-tint skipped —`, error instanceof Error ? error.message : error);
            unmountShaderField(host);
        }
    }
});
