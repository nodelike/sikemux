export function durationLabel(ms: number): string {
    // Tool calls are often quicker than a tenth of a second, and rounding those
    // to seconds reported every one of them as the same 0.0s.
    const elapsed = Math.max(0, ms);
    if (elapsed < 1000) return `${Math.round(elapsed)}ms`;
    return elapsedLabel(Math.round(elapsed / 1000));
}

export function elapsedLabel(seconds: number): string {
    if (seconds < 60) return `${seconds}s`;
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${String(seconds % 60).padStart(2, "0")}s`;
    return `${Math.floor(seconds / 3600)}h ${String(Math.floor(seconds / 60) % 60).padStart(2, "0")}m`;
}
