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

export type ShaderFieldPreset = "pane" | "onboarding";

/*
 * The content area's surface, and room for the tour.
 *
 * One each, because neither is per-pane: splitting the window no longer costs a
 * context. That matters because every terminal takes one of the page's ~16 for
 * its own renderer, and a terminal losing that to a decoration is a far worse
 * trade than a surface without a texture.
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
 * Why the last mount did not happen. Every refusal here is deliberate and
 * silent — a field is decoration and must never raise — which also made a blank
 * surface impossible to tell apart from a broken one. This is surfaced through
 * `browserDiagnostics()` so the answer is one panel away.
 */
let lastRefusal: string | null = null;
/*
 * Time zero for continuous presets. Read once when the module loads so every
 * surface — mounted now or twenty tab switches later — derives the same phase
 * from the same origin.
 */
const FIELD_EPOCH = performance.now();

/*
 * Whether this engine has WebGL 2 at all — asked without allocating anything.
 *
 * This used to probe by actually creating a context and handing it back, and
 * cache whatever it got. That was a permanent failure waiting to happen: panes
 * mount while the terminals are bringing up their own WebGL renderers, and if
 * the page is momentarily at its context limit then `getContext` returns null,
 * the `false` gets cached, and no field mounts again for the rest of the
 * session. A transient race turned into a dead feature.
 *
 * Checking for the constructor answers the only question worth caching — does
 * this engine do WebGL 2 — and can never be false because something else is
 * busy. Real failures are the ShaderMount call's business, and it is wrapped.
 */
function webglAvailable(): boolean {
    webglSupported ??= typeof WebGL2RenderingContext !== "undefined";
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
    /**
     * Join the shared clock instead of starting at zero, so a surface that is
     * released and rebuilt picks the animation up where it now is.
     */
    continuous?: boolean;
    uniforms: ShaderMountUniforms;
    needsNoise?: boolean;
}

const PRESETS: Record<ShaderFieldPreset, (runtime: Runtime, theme: Theme) => Recipe> = {
    /*
     * The content panes: two-colour dithering.
     *
     * A Bayer grid over simplex noise, the accent on the shell's own ground, so
     * it stays inside the theme and reads as texture rather than as colour. The
     * grain gradient was here first and washed the pane in four hues, which is
     * a different thing entirely — this is the dither.
     */
    pane: (runtime, theme) => ({
        /*
         * Moves, but never restarts.
         *
         * Only an on-screen pane may hold a WebGL context, so a pane's field is
         * released on tab switch and rebuilt when you come back. Starting each
         * rebuild at frame zero made that visible — a background nobody should
         * notice announced itself every time you changed tabs. Making it static
         * hid the rebuild but cost the motion, which was the wrong half to give
         * up. On the shared clock it does both: the pattern is always where the
         * clock says it should be, so a rebuild lands mid-drift and cannot be
         * told from a surface that was there all along.
         */
        fragmentShader: runtime.ditheringFragmentShader,
        speed: 0.35,
        continuous: true,
        uniforms: {
            u_colorBack: runtime.getShaderColorFromString(theme.chrome.bgDim),
            u_colorFront: runtime.getShaderColorFromString(theme.chrome.acc),
            u_shape: runtime.DitheringShapes.simplex,
            u_type: runtime.DitheringTypes["4x4"],
            u_pxSize: 2,
            ...sizing(runtime, "none", 0.55),
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
    if (surfaces.has(host)) return;
    if (!webglAvailable()) {
        lastRefusal = "no webgl2 context";
        return;
    }
    prune();
    if (surfaces.size >= SURFACE_BUDGET) {
        lastRefusal = `budget spent (${SURFACE_BUDGET} surfaces live)`;
        return;
    }
    lastRefusal = null;

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
            /*
             * `frame` is in the same accumulated units the runtime advances by
             * (`currentFrame += dt * speed`), so sharing a phase means scaling
             * the elapsed time by this preset's own speed rather than passing
             * raw milliseconds.
             */
            const phase = recipe.continuous ? (performance.now() - FIELD_EPOCH) * recipe.speed : 0;
            const mount = new runtime.ShaderMount(
                host,
                recipe.fragmentShader,
                recipe.uniforms,
                { antialias: false },
                animate ? recipe.speed : 0,
                // Asked for less motion, a continuous preset holds the phase it
                // would have had rather than snapping to an arbitrary still.
                animate ? phase : phase || 2500,
                // These are soft fields behind text, not artwork. Rendering at
                // 1x rather than the default 2x halves the fill cost and is
                // invisible through the mask.
                1,
            );
            surfaces.set(host, { preset, mount, runtime });
            host.dataset.shaderField = preset;
        } catch (error) {
            const reason = error instanceof Error ? error.message : String(error);
            console.warn(`Paper Shaders: ${preset} field skipped —`, reason);
            lastRefusal = `${preset}: ${reason}`;
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

/** What the shader fields are doing, for the diagnostics panel. */
export function shaderFieldDiagnostics(): Record<string, unknown> {
    return {
        live: surfaces.size,
        budget: SURFACE_BUDGET,
        webgl2: webglSupported,
        presets: [...surfaces.values()].map((surface) => `${surface.preset}${surface.mount ? "" : " (pending)"}`),
        animating: shouldAnimate(),
        lastRefusal,
    };
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
