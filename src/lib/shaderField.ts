/*
 * Paper Shaders, used only where the workspace has nothing to show yet.
 *
 * Two things are scarce here: WebGL contexts and frames.
 *
 * Contexts, because a page is capped at roughly sixteen before the browser
 * starts evicting the oldest without warning, and a terminal asking for its own
 * WebGL renderer must always win that race. So the budget below is deliberately
 * small, the runtime is fetched on first use, and a surface is released the
 * moment its host leaves the document.
 *
 * Frames, because every one of these is a full-screen fragment shader on a
 * see-through window, which the window server has to recomposite. So the
 * runtime's own frame loop is off and `advance` below drives every live surface
 * from one timer, at a rate the machine can afford, and only while there is
 * someone looking at it.
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
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Theme } from "../themes";
import { onBatteryPower } from "../state/battery";
import { currentTheme, subscribeTheme } from "../themes/bus";
import { prefersReducedMotion } from "./motion";

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

export type ShaderFieldPreset = "ambient" | "onboarding" | "release";

/*
 * The panes on the screen being read, plus room for the tour.
 *
 * The texture belongs to a pane again — a pane is a surface now, and a surface
 * without its own texture is the thing that made a split read as one card cut
 * in half. So splitting costs a context again, and the number below is the cap
 * on how many.
 *
 * It stays well under the page's ~16 because a terminal asks for one as soon
 * as its WebGL renderer is switched on, and a terminal losing that to a
 * decoration is a far worse trade than a surface without a texture. Only panes
 * on the live screen ask, and the refusal path below is what keeps the trade
 * on the right side when a layout goes wider than this.
 */
const SURFACE_BUDGET = 6;

/*
 * How often a field is repainted. Nothing here is being read, so the motion
 * only has to read as drift; below about fifteen it starts to look like a
 * stutter instead. The slower rate is for a machine paying for every frame out
 * of its battery.
 */
const FRAMES_PER_SECOND = 30;
const BATTERY_FRAMES_PER_SECOND = 20;

interface Surface {
    preset: ShaderFieldPreset;
    mount: InstanceType<Shaders["ShaderMount"]> | null;
    runtime: Runtime | null;
    /** What `advance` multiplies elapsed time by. Zero for a still. */
    speed: number;
    /** The moment this surface's clock reads zero. */
    origin: number;
    resize: ResizeObserver | null;
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
    return !prefersReducedMotion();
}

/** The frame a still holds, for a preset whose own clock starts at zero. */
const STILL_FRAME = 2500;

/**
 * How many pixels a field may paint: one for each CSS pixel of its host.
 *
 * The seventh `ShaderMount` argument looks like a cap and is not — it is
 * `minPixelRatio`, a floor, so passing 1 leaves a Retina screen rendering at 2x
 * and paying four times the fill. The eighth argument is the real limit: the
 * runtime divides the canvas down until it fits. A host's own CSS area is the
 * number that makes that division land on exactly 1x.
 */
export function shaderFieldPixelCap(width: number, height: number): number {
    return Math.max(1, Math.round(width) * Math.round(height));
}

function hostPixelCap(host: HTMLElement): number {
    const rect = host.getBoundingClientRect();
    return shaderFieldPixelCap(rect.width, rect.height);
}

/*
 * Whether the app is the window the user is looking at.
 *
 * The webview's own `blur` is not the question being asked: focus moving to a
 * native child webview — a browser tab — blurs the page while the window is
 * still very much in front of the reader. Tauri answers for the window itself,
 * and the page events are only the fallback for a build running without it.
 */
let windowFocused = true;
let focusWatchStarted = false;

function setFocused(focused: boolean): void {
    if (windowFocused === focused) return;
    windowFocused = focused;
    syncTicker();
}

function startFocusWatch(): void {
    if (focusWatchStarted) return;
    focusWatchStarted = true;
    document.addEventListener("visibilitychange", syncTicker);
    try {
        void getCurrentWindow()
            .onFocusChanged(({ payload }) => setFocused(payload))
            .catch(watchPageFocusInstead);
    } catch {
        watchPageFocusInstead();
    }
}

function watchPageFocusInstead(): void {
    window.addEventListener("focus", () => setFocused(true));
    window.addEventListener("blur", () => setFocused(false));
}

let tickerHandle: number | null = null;

function frameIntervalMs(): number {
    return Math.round(1000 / (onBatteryPower() ? BATTERY_FRAMES_PER_SECOND : FRAMES_PER_SECOND));
}

function shouldTick(): boolean {
    if (!windowFocused || document.hidden) return false;
    for (const surface of surfaces.values()) if (surface.mount && surface.speed !== 0) return true;
    return false;
}

/*
 * Every surface reads its frame off the clock rather than accumulating one, so
 * a field that was paused, released and rebuilt is always exactly where the
 * clock says it should be and a resume cannot be told from a surface that was
 * running all along.
 */
function advance(): void {
    const now = performance.now();
    for (const [host, surface] of surfaces) {
        if (!surface.mount || surface.speed === 0) continue;
        try {
            surface.mount.setFrame((now - surface.origin) * surface.speed);
        } catch (error) {
            console.warn(`Paper Shaders: ${surface.preset} frame skipped —`, error instanceof Error ? error.message : error);
            unmountShaderField(host);
        }
    }
}

function tick(): void {
    tickerHandle = null;
    advance();
    scheduleTick();
}

function scheduleTick(): void {
    if (tickerHandle === null && shouldTick()) tickerHandle = window.setTimeout(tick, frameIntervalMs());
}

function syncTicker(): void {
    if (shouldTick()) {
        scheduleTick();
        return;
    }
    if (tickerHandle !== null) {
        window.clearTimeout(tickerHandle);
        tickerHandle = null;
    }
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

const TRANSPARENT: [number, number, number, number] = [0, 0, 0, 0];

function lightDotColor(runtime: Runtime, theme: Theme): [number, number, number, number] {
    const hairline = runtime.getShaderColorFromString(theme.chrome.line);
    const ink = runtime.getShaderColorFromString(theme.chrome.inkMuted);
    return [0, 1, 2].map((i) => hairline[i] * 0.9 + ink[i] * 0.1).concat(1) as [number, number, number, number];
}

/*
 * The dithering shader lights solid wherever its noise peaks and draws in one
 * colour. The release sky squeezes the noise into a band, so it neither fills
 * solid nor empties out, and takes each dot's colour from a gradient running across the sky.
 */
const RELEASE_DENSITY = { floor: 0.12, peak: 0.42 };

function patchShader(shader: string, edits: readonly [string, string][]): string {
    return edits.reduce((source, [from, to]) => {
        if (!source.includes(from)) throw new Error(`the dithering shader no longer contains "${from}"`);
        return source.replace(from, to);
    }, shader);
}

function releaseSkyShader(shader: string): string {
    return patchShader(shader, [
        ["uniform float u_shape;", "uniform float u_shape;\nuniform vec4 u_colors[3];"],
        [
            "float res = step(.5, shape + dithering);",
            `float res = step(.5, mix(${RELEASE_DENSITY.floor.toFixed(2)}, ${RELEASE_DENSITY.peak.toFixed(2)}, shape) + dithering);`,
        ],
        [
            "vec3 fgColor = u_colorFront.rgb * u_colorFront.a;",
            `float hueAt = clamp(normalizedUV.x + .5 + .12 * sin(t + normalizedUV.y * 4.), 0., 1.);
  vec3 hue = hueAt < .5 ? mix(u_colors[0].rgb, u_colors[1].rgb, hueAt * 2.) : mix(u_colors[1].rgb, u_colors[2].rgb, hueAt * 2. - 1.);
  vec3 fgColor = hue * u_colorFront.a;`,
        ],
    ]);
}

const PRESETS: Record<ShaderFieldPreset, (runtime: Runtime, theme: Theme) => Recipe> = {
    /*
     * The screen's surface: a Bayer grid over simplex noise, so the card being
     * read carries grain instead of a flat fill.
     *
     * Only the dots are painted; the ground between them is whatever the card
     * already shows. It used to paint the theme's recess there too, and on a
     * see-through window — where the card paints nothing — that recess was the
     * only fill on the screen, so the mask turned it into a dark wash sliding
     * down over the desktop. On a dark theme the dots are the raised surface
     * tone. On a light one every surface tone is too close to the ground to
     * show and the muted ink is too loud, so they are the hairline nudged a
     * tenth of the way toward that ink.
     */
    ambient: (runtime, theme) => ({
        /*
         * Moves, but never restarts.
         *
         * The host outlives every pane now, so this rarely rebuilds — but it
         * still does when the shell remounts, and starting each rebuild at
         * frame zero made that visible: a background nobody should notice
         * announced itself. On the shared clock the pattern is always where the
         * clock says it should be, so a rebuild lands mid-drift and cannot be
         * told from a surface that was there all along.
         */
        fragmentShader: runtime.ditheringFragmentShader,
        // Over one card rather than the whole window the drift covers less ground,
        // so it has to move quicker to read as weather instead of as a still.
        speed: 0.5,
        continuous: true,
        uniforms: {
            u_colorBack: TRANSPARENT,
            u_colorFront: theme.dark ? runtime.getShaderColorFromString(theme.chrome.bgRaised) : lightDotColor(runtime, theme),
            u_shape: runtime.DitheringShapes.simplex,
            u_type: runtime.DitheringTypes["8x8"],
            // The dots are the texture, and their size is free: the shader
            // quantizes each fragment against this whether it is 2 or 8, and
            // the noise behind it is evaluated once per fragment either way.
            // 2 washed into haze, 4 read as blocks; this sits between them.
            u_pxSize: 3,
            ...sizing(runtime, "none", 2.4),
        },
    }),

    /*
     * The sky over the release notes' sidebar: the ambient grain running from
     * the accent through the syntax pink to the syntax green. Its host fades it
     * out down the sidebar.
     */
    release: (runtime, theme) => ({
        fragmentShader: releaseSkyShader(runtime.ditheringFragmentShader),
        speed: 0.35,
        uniforms: {
            u_colorBack: TRANSPARENT,
            u_colorFront: runtime.getShaderColorFromString(theme.chrome.acc),
            u_colors: [theme.chrome.acc, theme.highlight.function, theme.highlight.string].map((hue) => runtime.getShaderColorFromString(hue)),
            u_shape: runtime.DitheringShapes.simplex,
            u_type: runtime.DitheringTypes["8x8"],
            u_pxSize: 3,
            ...sizing(runtime, "none", 1.4),
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

    startFocusWatch();
    // Claimed before the first await so a burst of calls mounts once.
    surfaces.set(host, { preset, mount: null, runtime: null, speed: 0, origin: FIELD_EPOCH, resize: null });
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
            const origin = recipe.continuous ? FIELD_EPOCH : performance.now();
            /*
             * A frame is in the units the runtime accumulates in
             * (`currentFrame += dt * speed`), so joining a clock means scaling
             * elapsed time by this preset's own speed rather than passing raw
             * milliseconds.
             */
            const phase = (performance.now() - origin) * recipe.speed;
            const mount = new runtime.ShaderMount(
                host,
                recipe.fragmentShader,
                recipe.uniforms,
                { antialias: false },
                // The runtime's frame loop stays off whatever happens: `advance`
                // drives every surface, so there is one timer rather than one
                // rAF chain per field.
                0,
                // Asked for less motion, a field holds the phase it would have
                // had rather than snapping to an arbitrary still.
                animate ? phase : phase || STILL_FRAME,
                1,
                hostPixelCap(host),
            );
            const resize = trackHostSize(host, mount);
            surfaces.set(host, { preset, mount, runtime, speed: animate ? recipe.speed : 0, origin, resize });
            host.dataset.shaderField = preset;
            syncTicker();
        } catch (error) {
            const reason = error instanceof Error ? error.message : String(error);
            console.warn(`Paper Shaders: ${preset} field skipped —`, reason);
            lastRefusal = `${preset}: ${reason}`;
            surfaces.delete(host);
        }
    })();
}

/*
 * The cap is an area, so it has to be recomputed whenever the host changes
 * shape — a pane dragged wider would otherwise keep painting at the old area
 * spread over more pixels, which is a scale below 1x and a visibly softer
 * texture.
 */
function trackHostSize(host: HTMLElement, mount: InstanceType<Shaders["ShaderMount"]>): ResizeObserver | null {
    if (typeof ResizeObserver === "undefined") return null;
    let cap = hostPixelCap(host);
    const observer = new ResizeObserver(() => {
        const next = hostPixelCap(host);
        if (next === cap) return;
        cap = next;
        mount.setMaxPixelCount(next);
    });
    observer.observe(host);
    return observer;
}

/** Release a surface and its WebGL context. Safe to call for a host that never got one. */
export function unmountShaderField(host: HTMLElement): void {
    const surface = surfaces.get(host);
    if (!surface) return;
    surfaces.delete(host);
    delete host.dataset.shaderField;
    surface.resize?.disconnect();
    surface.mount?.dispose();
    syncTicker();
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
        ticking: tickerHandle !== null,
        framesPerSecond: Math.round(1000 / frameIntervalMs()),
        windowFocused,
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
