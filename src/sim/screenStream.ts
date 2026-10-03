import { simApi, type SimStreamFormat } from "../api/sim";
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

export interface ScreenStreamEvents {
    /** Frames drawn in the last second. */
    onFps: (fps: number) => void;
    /** From a touch going out to the next frame drawn, in milliseconds. */
    onLatency?: (ms: number) => void;
    onFormat: (format: SimStreamFormat) => void;
    onError: (message: string) => void;
}

/**
 * Plays a device's screen into a canvas until stopped. The frames come through
 * the app on a Tauri channel. Nothing else depends on it: taps, screenshots and
 * the accessibility tree work with no stream running.
 */
export function playScreen(udid: string, canvas: HTMLCanvasElement, events: ScreenStreamEvents): { stop: () => void; markInput: () => void } {
    const context = canvas.getContext("2d");
    let stopped = false;
    let watch: Promise<number> | null = null;
    let inputAt: number | null = null;
    let decoder: VideoDecoder | null = null;
    let format: SimStreamFormat = "h264";
    let drawn = 0;
    const fpsTimer = window.setInterval(() => {
        events.onFps(drawn);
        drawn = 0;
    }, 1000);

    const draw = (image: CanvasImageSource, width: number, height: number) => {
        if (!context) return;
        if (canvas.width !== width || canvas.height !== height) {
            canvas.width = width;
            canvas.height = height;
        }
        context.drawImage(image, 0, 0, width, height);
        drawn += 1;
        if (inputAt !== null) {
            events.onLatency?.(performance.now() - inputAt);
            inputAt = null;
        }
    };

    const open = async (wanted: SimStreamFormat) => {
        format = wanted;
        events.onFormat(wanted);
        watch = simApi.watch(udid, wanted, (frame) => void receive(new Uint8Array(frame)));
        await watch;
    };

    const close = () => {
        if (!watch) return;
        void watch.then((id) => simApi.unwatch(id)).catch(() => {});
        watch = null;
    };

    const fallBackToMjpeg = (reason: string) => {
        if (format === "mjpeg" || stopped) return;
        console.warn(`simulator screen: ${reason}; showing MJPEG instead`);
        if (decoder && decoder.state !== "closed") decoder.close();
        decoder = null;
        close();
        void simApi.stopStream(udid, "h264").catch(() => {});
        void open("mjpeg").catch((error) => events.onError(String(error)));
    };

    let choice: DecoderChoice | null = null;
    let configuring = false;
    const receive = async (bytes: Uint8Array) => {
        if (stopped) return;
        if (format === "mjpeg") {
            const image = await createImageBitmap(new Blob([bytes.slice()], { type: "image/jpeg" }));
            draw(image, image.width, image.height);
            image.close();
            return;
        }
        const units = splitUnits(bytes);
        const key = isKeyFrame(units);
        if (!decoder) {
            const sps = units.find((unit) => unitType(unit) === SPS);
            const pps = units.find((unit) => unitType(unit) === PPS);
            if (!key || !sps || !pps || configuring) return;
            configuring = true;
            choice = await chooseDecoder(sps, pps, webCodecsSupports);
            if (choice.kind === "mjpeg") return fallBackToMjpeg("no H.264 decoder for this stream");
            decoder = new VideoDecoder({
                output: (frame) => {
                    draw(frame, frame.displayWidth, frame.displayHeight);
                    frame.close();
                },
                error: (error) => fallBackToMjpeg(`the H.264 decoder failed: ${error.message}`),
            });
            decoder.configure(choice.config);
        }
        if (decoder.state !== "configured") return;
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

    void open("h264").catch((error) => events.onError(String(error)));

    return {
        stop: () => {
            stopped = true;
            window.clearInterval(fpsTimer);
            close();
            if (decoder && decoder.state !== "closed") decoder.close();
            void simApi.stopStream(udid).catch(() => {});
        },
        markInput: () => {
            inputAt = performance.now();
        },
    };
}
