import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { simApi } from "../api/sim";
import { playScreen, turnTransform } from "./screenStream";

vi.mock("../api/sim", () => ({
    simApi: { watch: vi.fn(), unwatch: vi.fn() },
}));

const canvas = { getContext: () => null } as unknown as HTMLCanvasElement;
const keyFrame = Uint8Array.of(0, 0, 0, 1, 0x67, 0x64, 0x00, 0x1f, 0, 0, 0, 1, 0x68, 0xee, 0, 0, 0, 1, 0x65, 0x88).buffer;
const deltaFrame = Uint8Array.of(0, 0, 0, 1, 0x41, 0x9a).buffer;

class FakeDecoder {
    static made: FakeDecoder[] = [];
    static isConfigSupported = vi.fn<(config: VideoDecoderConfig) => Promise<{ supported: boolean }>>();
    state = "unconfigured";
    decodeQueueSize = 0;
    decoded: string[] = [];
    constructor() {
        FakeDecoder.made.push(this);
    }
    configure() {
        this.state = "configured";
    }
    decode(chunk: { type: string }) {
        this.decoded.push(chunk.type);
    }
    close() {
        this.state = "closed";
    }
}

let sendFrame: (frame: ArrayBuffer) => void = () => {};
let endWatch: (reason: string) => void = () => {};

beforeEach(() => {
    FakeDecoder.made = [];
    FakeDecoder.isConfigSupported.mockResolvedValue({ supported: true });
    vi.stubGlobal("VideoDecoder", FakeDecoder);
    vi.stubGlobal(
        "EncodedVideoChunk",
        class {
            type: string;
            constructor(init: { type: string }) {
                this.type = init.type;
            }
        },
    );
    vi.mocked(simApi.watch).mockImplementation(async (_udid, _format, onFrame, onEnd) => {
        sendFrame = onFrame;
        endWatch = (reason) => onEnd?.(reason);
        return 7;
    });
    vi.mocked(simApi.unwatch).mockResolvedValue(undefined);
});

afterEach(() => {
    vi.unstubAllGlobals();
    vi.clearAllMocks();
});

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("playing a device's screen", () => {
    it("leaves the stream to the helper when it stops, and only stops watching", async () => {
        const player = playScreen("UDID", canvas, { onError: vi.fn() });
        await settle();
        player.stop();
        await settle();
        expect(simApi.unwatch).toHaveBeenCalledWith(7);
    });

    it("says when the stream stops on its own, and not once it was stopped", async () => {
        const onEnded = vi.fn();
        const player = playScreen("UDID", canvas, { onError: vi.fn(), onEnded });
        await settle();
        endWatch("the helper stopped");
        expect(onEnded).toHaveBeenCalledWith("the helper stopped");

        player.stop();
        endWatch("again");
        expect(onEnded).toHaveBeenCalledTimes(1);
    });

    it("falls back to JPEG frames when the helper cannot stream H.264", async () => {
        vi.mocked(simApi.watch).mockRejectedValueOnce({ reason: "streamUnavailable", message: "no encoder" });
        const onError = vi.fn();
        playScreen("UDID", canvas, { onError });
        await settle();
        expect(vi.mocked(simApi.watch).mock.calls.map((call) => call[1])).toEqual(["h264", "mjpeg"]);
        expect(onError).not.toHaveBeenCalled();
    });

    it("makes no decoder when it stops while one is being chosen", async () => {
        let answer: (value: { supported: boolean }) => void = () => {};
        FakeDecoder.isConfigSupported.mockReturnValue(new Promise((resolve) => (answer = resolve)));
        const player = playScreen("UDID", canvas, { onError: vi.fn() });
        await settle();
        sendFrame(keyFrame);
        player.stop();
        answer({ supported: true });
        await settle();
        expect(FakeDecoder.made).toEqual([]);
    });

    it("decodes every frame even while the decoder is behind, since a skipped one would hold the picture until the next key frame", async () => {
        playScreen("UDID", canvas, { onError: vi.fn() });
        await settle();
        sendFrame(keyFrame);
        await settle();
        const decoder = FakeDecoder.made[0];
        decoder.decodeQueueSize = 5;
        sendFrame(deltaFrame);
        sendFrame(deltaFrame);
        await settle();
        expect(decoder.decoded).toEqual(["key", "delta", "delta"]);
    });
});

describe("turning the picture with the device", () => {
    const place = (orientation: Parameters<typeof turnTransform>[0], x: number, y: number) => {
        const [a, b, c, d, e, f] = turnTransform(orientation, 1206, 2622);
        return [a * x + c * y + e, b * x + d * y + f];
    };

    it("puts the upright frame's corners where a turned device shows them", () => {
        expect(place("portrait", 1206, 0)).toEqual([1206, 0]);
        expect(place("landscapeLeft", 1206, 0)).toEqual([0, 0]);
        expect(place("landscapeLeft", 0, 0)).toEqual([0, 1206]);
        expect(place("landscapeRight", 0, 2622)).toEqual([0, 0]);
        expect(place("portraitUpsideDown", 1206, 2622)).toEqual([0, 0]);
    });
});
