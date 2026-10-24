export function splitMulti(value: string, delimiter: string): string[] {
    return value
        .split(delimiter)
        .map((part) => part.trim())
        .filter(Boolean);
}
