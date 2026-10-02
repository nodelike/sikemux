export function durationLabel(ms: number): string {
    // Tool calls are often quicker than a tenth of a second, and rounding those
    // to seconds reported every one of them as the same 0.0s.
    const elapsed = Math.max(0, ms);
    if (elapsed < 1000) return `${Math.round(elapsed)}ms`;
    const seconds = Math.round(elapsed / 1000);
    return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${String(seconds % 60).padStart(2, "0")}s`;
}

export function elapsedLabel(seconds: number): string {
    return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${String(seconds % 60).padStart(2, "0")}s`;
}
