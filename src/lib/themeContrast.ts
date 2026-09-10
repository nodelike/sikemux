function rgb(color: string): number[] {
    const hex = color.slice(1);
    const value =
        hex.length === 3
            ? hex
                  .split("")
                  .map((part) => part + part)
                  .join("")
            : hex;
    return [0, 2, 4].map((offset) => parseInt(value.slice(offset, offset + 2), 16));
}

export function contrastRatio(a: string, b: string): number {
    const luminance = (value: string) =>
        rgb(value)
            .map((channel) => {
                const c = channel / 255;
                return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
            })
            .reduce((sum, c, index) => sum + c * [0.2126, 0.7152, 0.0722][index], 0);
    const first = luminance(a),
        second = luminance(b);
    return (Math.max(first, second) + 0.05) / (Math.min(first, second) + 0.05);
}

export function readableColor(color: string, backgrounds: string[], ink: string, minimum = 4.6): string {
    const start = rgb(color),
        end = rgb(ink);
    if ([...start, ...end, ...backgrounds.flatMap(rgb)].some((channel) => !Number.isFinite(channel))) return ink;
    for (let step = 0; step <= 100; step++) {
        const candidate =
            "#" +
            start
                .map((channel, index) =>
                    Math.round(channel + ((end[index] - channel) * step) / 100)
                        .toString(16)
                        .padStart(2, "0"),
                )
                .join("");
        if (backgrounds.every((background) => contrastRatio(candidate, background) >= minimum)) return candidate;
    }
    return ink;
}
