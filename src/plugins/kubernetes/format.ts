import type { Pod, Workload } from "./api";

export type Tone = "live" | "warn" | "danger" | "idle";

/** The colour a pod's status reads in: trouble red, waiting amber, done grey. */
export function podTone(pod: Pick<Pod, "status" | "ready" | "containers">): Tone {
    const status = pod.status;
    if (/BackOff|Err|Error|OOMKilled|Failed|Evicted|Unknown|Invalid/i.test(status)) return "danger";
    if (status === "Completed" || status === "Succeeded") return "idle";
    if (status === "Running") return pod.ready >= pod.containers ? "live" : "warn";
    return "warn";
}

export function workloadTone(workload: Pick<Workload, "health">): Tone {
    return workload.health === "failed" ? "danger" : workload.health === "warning" || workload.health === "progressing" ? "warn" : "live";
}

/** How long ago an RFC 3339 time was, as kubectl's AGE column writes it: 45s, 12m, 3h, 6d. */
export function age(at: string | null, now = Date.now()): string {
    if (!at) return "—";
    const seconds = Math.max(0, Math.floor((now - Date.parse(at)) / 1000));
    if (Number.isNaN(seconds)) return "—";
    if (seconds < 120) return `${seconds}s`;
    if (seconds < 7200) return `${Math.floor(seconds / 60)}m`;
    if (seconds < 172_800) return `${Math.floor(seconds / 3600)}h`;
    return `${Math.floor(seconds / 86_400)}d`;
}

/** An image as people say it, without the registry's library prefix. */
export function shortImage(image: string): string {
    return image.replace(/^docker\.io\//, "").replace(/^library\//, "");
}

/** CPU in cores as people say it: 4, 3.8 or 250m. */
export function cores(millis: number): string {
    if (millis < 1000) return `${millis}m`;
    const whole = millis / 1000;
    return Number.isInteger(whole) ? String(whole) : whole.toFixed(1).replace(/\.0$/, "");
}

/** Memory as people say it: 512 MiB or 5.8 GiB. */
export function memory(bytes: number): string {
    const gib = bytes / 1024 ** 3;
    if (gib >= 1) return `${gib.toFixed(1).replace(/\.0$/, "")} GiB`;
    return `${Math.round(bytes / 1024 ** 2)} MiB`;
}
