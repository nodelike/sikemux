import { simApi, type SimScreen, type SimStreamFormat } from "../api/sim";
import { avcDescription, codecName, isKeyFrame, lengthPrefixed, PPS, SPS, splitUnits, unitType } from "./h264";

export type DecoderChoice = { kind: "annexb"; config: VideoDecoderConfig } | { kind: "avcc"; config: VideoDecoderConfig } | { kind: "mjpeg" };

/** Frames as they come where the decoder takes them; otherwise length-prefixed with an avcC description; otherwise MJPEG. */
export async function chooseDecoder(
    sps: Uint8Array,
    pps: Uint8Array,
    isSupported: (config: VideoDecoderConfig) => Promise<boolean>,
): Promise<DecoderChoice> {
    const codec = codecName(sps);
    const annexb: VideoDecoderConfig = { codec, optimizeForLatency: true };
    if (await isSupported(annexb)) return { kind: "annexb", config: annexb };
    const avcc: VideoDecoderConfig = { codec, optimizeForLatency: true, description: avcDescription(sps, pps) };
    if (await isSupported(avcc)) return { kind: "avcc", config: avcc };
    return { kind: "mjpeg" };
}

const webCodecsSupports = async (config: VideoDecoderConfig) =>
    typeof VideoDecoder !== "undefined" && (await VideoDecoder.isConfigSupported(config).catch(() => ({ supported: false }))).supported === true;

const unavailable = (error: unknown) => (error instanceof Error ? error.message : (JSON.stringify(error) ?? "")).includes("streamUnavailable");

export interface ScreenStreamEvents {
    /** Frames drawn in the last second. */
    onFps?: (fps: number) => void;
    /** From a touch going out to the next frame drawn, in milliseconds. */
    onLatency?: (ms: number) => void;
    onFormat?: (format: SimStreamFormat) => void;
    onFirstFrame?: () => void;
    /** The stream stopped on its own; a new `playScreen` carries on. */
    onEnded?: (reason: string) => void;
    onError: (message: string) => void;
}

export interface ScreenPlayer {
    stop: () => void;
    markInput: () => void;
    /** Shows the screen turned the way the device is; the frames themselves always come upright. */
    turn: (orientation: SimScreen["orientation"]) => void;
    /** Clips the screen to the device's own outline, given upright like the frames. */
    clip: (mask: CanvasImageSource | null) => void;
}

/** The transform that draws an upright frame `width` × `height` turned the way the device is. */
export function turnTransform(
    orientation: SimScreen["orientation"],
    width: number,
    height: number,
): [number, number, number, number, number, number] {
    switch (orientation) {
        case "landscapeLeft":
            return [0, -1, 1, 0, 0, width];
        case "landscapeRight":
            return [0, 1, -1, 0, height, 0];
        case "portraitUpsideDown":
            return [-1, 0, 0, -1, width, height];
        default:
            return [1, 0, 0, 1, 0, 0];
    }
}

/**
 * Plays a device's screen into a canvas until stopped. The frames come through
 * the app on a Tauri channel. Nothing else depends on it: taps, screenshots and
 * the accessibility tree work with no stream running.
 */
export function playScreen(udid: string, canvas: HTMLCanvasElement, events: ScreenStreamEvents): ScreenPlayer {
    const context = canvas.getContext("2d");
    const upright = document.createElement("canvas");
    const uprightContext = upright.getContext("2d");
    let orientation: SimScreen["orientation"] = "portrait";
    let mask: CanvasImageSource | null = null;
    let stopped = false;
    let watch: Promise<number> | null = null;
    let inputAt: number | null = null;
    let decoder: VideoDecoder | null = null;
    let choice: DecoderChoice | null = null;
    let configuring = false;
    let decodingImage = false;
    let format: SimStreamFormat = "h264";
    let drawn = 0;
    let framed = false;
    const onFps = events.onFps;
    const fpsTimer = onFps
        ? window.setInterval(() => {
              onFps(drawn);
              drawn = 0;
          }, 1000)
        : null;

    let shown = { width: 0, height: 0 };
    const sizes =
        typeof ResizeObserver === "undefined"
            ? null
            : new ResizeObserver(([entry]) => {
                  const box = entry.contentRect;
                  shown = { width: box.width * window.devicePixelRatio, height: box.height * window.devicePixelRatio };
                  paint();
              });
    sizes?.observe(canvas);

    const paint = () => {
        if (!context || !upright.width) return;
        const sideways = orientation.startsWith("landscape");
        const width = sideways ? upright.height : upright.width;
        const height = sideways ? upright.width : upright.height;
        const fit = shown.width && shown.height ? Math.min(1, shown.width / width, shown.height / height) : 1;
        const pixelsWide = Math.round(width * fit);
        const pixelsHigh = Math.round(height * fit);
        if (canvas.width !== pixelsWide || canvas.height !== pixelsHigh) {
            canvas.width = pixelsWide;
            canvas.height = pixelsHigh;
        }
        const scale = pixelsWide / width;
        const [a, b, c, d, e, f] = turnTransform(orientation, upright.width, upright.height);
        context.setTransform(a * scale, b * scale, c * scale, d * scale, e * scale, f * scale);
        context.imageSmoothingEnabled = true;
        context.imageSmoothingQuality = "high";
        context.drawImage(upright, 0, 0);
        if (mask) {
            context.globalCompositeOperation = "destination-in";
            context.drawImage(mask, 0, 0, upright.width, upright.height);
            context.globalCompositeOperation = "source-over";
        }
        context.setTransform(1, 0, 0, 1, 0, 0);
    };

    const draw = (image: CanvasImageSource, width: number, height: number) => {
        if (!uprightContext || stopped) return;
        if (upright.width !== width || upright.height !== height) {
            upright.width = width;
            upright.height = height;
        }
        uprightContext.drawImage(image, 0, 0, width, height);
        paint();
        if (!framed) {
            framed = true;
            events.onFirstFrame?.();
        }
        drawn += 1;
        if (inputAt !== null) {
            events.onLatency?.(performance.now() - inputAt);
            inputAt = null;
        }
    };

    const open = async (wanted: SimStreamFormat) => {
        format = wanted;
        events.onFormat?.(wanted);
        watch = simApi.watch(
            udid,
            wanted,
            (frame) => void receive(frame),
            (reason) => {
                if (stopped) return;
                watch = null;
                if (format === "h264" && unavailable(reason)) fallBackToMjpeg("the helper has no H.264 stream for this device");
                else events.onEnded?.(reason);
            },
        );
        await watch;
    };

    const close = () => {
        if (!watch) return;
        void watch.then((id) => simApi.unwatch(id)).catch(() => {});
        watch = null;
    };

    const closeDecoder = () => {
        if (decoder && decoder.state !== "closed") decoder.close();
        decoder = null;
    };

    const fallBackToMjpeg = (reason: string) => {
        if (format === "mjpeg" || stopped) return;
        console.warn(`simulator screen: ${reason}; showing MJPEG instead`);
        closeDecoder();
        close();
        void open("mjpeg").catch((error) => events.onError(String(error)));
    };

    const drawJpeg = async (frame: ArrayBuffer) => {
        if (decodingImage) return;
        decodingImage = true;
        try {
            const image = await createImageBitmap(new Blob([frame], { type: "image/jpeg" }));
            draw(image, image.width, image.height);
            image.close();
        } finally {
            decodingImage = false;
        }
    };

    const startDecoder = async (sps: Uint8Array, pps: Uint8Array) => {
        configuring = true;
        choice = await chooseDecoder(sps, pps, webCodecsSupports);
        if (stopped) return false;
        if (choice.kind === "mjpeg") {
            fallBackToMjpeg("no H.264 decoder for this stream");
            return false;
        }
        decoder = new VideoDecoder({
            output: (frame) => {
                draw(frame, frame.displayWidth, frame.displayHeight);
                frame.close();
            },
            error: (error) => fallBackToMjpeg(`the H.264 decoder failed: ${error.message}`),
        });
        decoder.configure(choice.config);
        return true;
    };

    const receive = async (frame: ArrayBuffer) => {
        if (stopped) return;
        if (format === "mjpeg") return drawJpeg(frame);
        const bytes = new Uint8Array(frame);
        const units = splitUnits(bytes);
        const key = isKeyFrame(units);
        if (!decoder) {
            const sps = units.find((unit) => unitType(unit) === SPS);
            const pps = units.find((unit) => unitType(unit) === PPS);
            if (!key || !sps || !pps || configuring) return;
            if (!(await startDecoder(sps, pps))) return;
        }
        if (!decoder || decoder.state !== "configured") return;
        try {
            decoder.decode(
                new EncodedVideoChunk({
                    type: key ? "key" : "delta",
                    timestamp: Math.round(performance.now() * 1000),
                    data: choice?.kind === "avcc" ? lengthPrefixed(units) : bytes,
                }),
            );
        } catch (error) {
            fallBackToMjpeg(`the H.264 decoder refused a frame: ${String(error)}`);
        }
    };

    void open("h264").catch((error) => {
        if (unavailable(error)) fallBackToMjpeg("the helper has no H.264 stream for this device");
        else events.onError(String(error));
    });

    return {
        stop: () => {
            stopped = true;
            sizes?.disconnect();
            if (fpsTimer !== null) window.clearInterval(fpsTimer);
            close();
            closeDecoder();
        },
        markInput: () => {
            inputAt = performance.now();
        },
        turn: (next) => {
            if (next === orientation) return;
            orientation = next;
            paint();
        },
        clip: (next) => {
            mask = next;
            paint();
        },
    };
}
