import { describe, expect, it } from "vitest";
import { devicePoint } from "./SimulatorPane";
import { chooseDecoder } from "./screenStream";

const screen = { width: 402, height: 874, scale: 3 };
/* A 1206 x 2622 frame drawn into a 600 x 600 box: scaled to fit its height and centred, with bars either side. */
const canvas = { width: 1206, height: 2622, rect: { left: 100, top: 50, width: 600, height: 600 } };

describe("pointing at the simulator's screen", () => {
    it("finds the device point under the pointer, past the bars a contained canvas leaves", () => {
        const drawnWidth = 1206 * (600 / 2622);
        const left = 100 + (600 - drawnWidth) / 2;
        expect(devicePoint(canvas, screen, left, 50)).toEqual({ x: 0, y: 0 });
        const centre = devicePoint(canvas, screen, 400, 350)!;
        expect(centre.x).toBeCloseTo(201);
        expect(centre.y).toBeCloseTo(437);
    });

    it("ignores the bars beside the screen and a canvas with nothing drawn yet", () => {
        expect(devicePoint(canvas, screen, 105, 300)).toBeNull();
        expect(devicePoint({ ...canvas, width: 0, height: 0 }, screen, 400, 350)).toBeNull();
    });
});

describe("decoding the screen", () => {
    const sps = Uint8Array.of(0x67, 0x64, 0x00, 0x1f);
    const pps = Uint8Array.of(0x68, 0xee);

    it("takes frames as they come when the decoder accepts that", async () => {
        expect(await chooseDecoder(sps, pps, async () => true)).toMatchObject({ kind: "annexb", config: { codec: "avc1.64001f" } });
    });

    it("hands over avcC when only that is accepted, and MJPEG when neither is", async () => {
        const avcc = await chooseDecoder(sps, pps, async (config) => config.description !== undefined);
        expect(avcc.kind).toBe("avcc");
        expect(await chooseDecoder(sps, pps, async () => false)).toEqual({ kind: "mjpeg" });
    });
});
