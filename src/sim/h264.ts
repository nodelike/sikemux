/** One encoded frame as the helper sends it: H.264 units, each after a 00 00 01 or 00 00 00 01 start code. */
export function splitUnits(frame: Uint8Array): Uint8Array[] {
    const starts: { at: number; length: number }[] = [];
    for (let i = 0; i + 2 < frame.length; i++) {
        if (frame[i] !== 0 || frame[i + 1] !== 0) continue;
        if (frame[i + 2] === 1) {
            starts.push({ at: i, length: 3 });
            i += 2;
        } else if (frame[i + 2] === 0 && frame[i + 3] === 1) {
            starts.push({ at: i, length: 4 });
            i += 3;
        }
    }
    return starts.map((start, index) => frame.subarray(start.at + start.length, starts[index + 1]?.at ?? frame.length));
}

export const unitType = (unit: Uint8Array): number => unit[0] & 0x1f;

export const SPS = 7;
export const PPS = 8;
export const KEY_FRAME = 5;

export const isKeyFrame = (units: Uint8Array[]): boolean => units.some((unit) => unitType(unit) === KEY_FRAME);

const hex = (byte: number) => byte.toString(16).padStart(2, "0");

/** The WebCodecs codec name for a stream, from its sequence parameter set: profile, constraints and level. */
export const codecName = (sps: Uint8Array): string => `avc1.${hex(sps[1])}${hex(sps[2])}${hex(sps[3])}`;

/** The avcC record a decoder takes as its description when frames come length-prefixed instead of after start codes. */
export function avcDescription(sps: Uint8Array, pps: Uint8Array): Uint8Array {
    const record = new Uint8Array(11 + sps.length + pps.length);
    record.set([1, sps[1], sps[2], sps[3], 0xff, 0xe1, sps.length >> 8, sps.length & 0xff]);
    record.set(sps, 8);
    record.set([1, pps.length >> 8, pps.length & 0xff], 8 + sps.length);
    record.set(pps, 11 + sps.length);
    return record;
}

/** The same units with a four-byte length before each, as avcC streams carry them; parameter sets stay in the description. */
export function lengthPrefixed(units: Uint8Array[]): Uint8Array {
    const kept = units.filter((unit) => unitType(unit) !== SPS && unitType(unit) !== PPS);
    const out = new Uint8Array(kept.reduce((total, unit) => total + 4 + unit.length, 0));
    let at = 0;
    for (const unit of kept) {
        new DataView(out.buffer).setUint32(at, unit.length);
        out.set(unit, at + 4);
        at += 4 + unit.length;
    }
    return out;
}
