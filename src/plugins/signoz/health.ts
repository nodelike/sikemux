export function percent(rate: number): string {
    if (rate === 0) return "0%";
    if (rate < 0.001) return "<0.1%";
    return `${(rate * 100).toFixed(rate >= 0.1 ? 0 : 1)}%`;
}

export function perMinute(count: number, minutes: number): string {
    const rate = count / Math.max(1, minutes);
    if (rate >= 10_000) return `${(rate / 1_000).toFixed(0)}k/min`;
    if (rate >= 1_000) return `${(rate / 1_000).toFixed(1)}k/min`;
    if (rate >= 10) return `${Math.round(rate)}/min`;
    return `${Number(rate.toPrecision(2))}/min`;
}
