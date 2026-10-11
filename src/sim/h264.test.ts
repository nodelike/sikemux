import { describe, expect, it } from "vitest";
import { avcDescription, codecName, isKeyFrame, lengthPrefixed, splitUnits, unitType } from "./h264";

const sps = Uint8Array.of(0x67, 0x64, 0x00, 0x1f, 0xac);
const pps = Uint8Array.of(0x68, 0xee, 0x3c);
const idr = Uint8Array.of(0x65, 0x88, 0x84);
const frame = Uint8Array.of(0, 0, 0, 1, ...sps, 0, 0, 1, ...pps, 0, 0, 0, 1, ...idr);

describe("H.264 from the simulator", () => {
    it("splits a frame at both lengths of start code", () => {
        expect(splitUnits(frame).map(unitType)).toEqual([7, 8, 5]);
        expect(splitUnits(frame)[2]).toEqual(idr);
    });

    it("knows a key frame from the rest", () => {
        expect(isKeyFrame(splitUnits(frame))).toBe(true);
        expect(isKeyFrame(splitUnits(Uint8Array.of(0, 0, 1, 0x41, 0x9a)))).toBe(false);
    });

    it("names the codec from the sequence parameter set", () => {
        expect(codecName(sps)).toBe("avc1.64001f");
    });

    it("builds the avcC record and length-prefixed frames for decoders that refuse start codes", () => {
        expect([...avcDescription(sps, pps)]).toEqual([1, 0x64, 0x00, 0x1f, 0xff, 0xe1, 0, 5, ...sps, 1, 0, 3, ...pps]);
        expect([...lengthPrefixed(splitUnits(frame))]).toEqual([0, 0, 0, 3, ...idr]);
    });
});
