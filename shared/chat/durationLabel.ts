import { english, type Say } from "../lib/say";

export function durationLabel(ms: number, say: Say = english): string {
    // Tool calls are often quicker than a tenth of a second, and rounding those
    // to seconds reported every one of them as the same 0.0s.
    const elapsed = Math.max(0, ms);
    if (elapsed < 1000) return say("{ms}ms", { ms: Math.round(elapsed) });
    return elapsedLabel(Math.round(elapsed / 1000), say);
}

export function elapsedLabel(seconds: number, say: Say = english): string {
    const two = (value: number) => String(value).padStart(2, "0");
    if (seconds < 60) return say("{seconds}s", { seconds });
    if (seconds < 3600) return say("{minutes}m {seconds}s", { minutes: Math.floor(seconds / 60), seconds: two(seconds % 60) });
    return say("{hours}h {minutes}m", { hours: Math.floor(seconds / 3600), minutes: two(Math.floor(seconds / 60) % 60) });
}
