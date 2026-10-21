import type { Container, Port } from "./api";

/** A published port as a browser would open it, such as `localhost:3000`. */
export function portLabel(port: Port): string {
    if (port.public === null) return `${port.private}/${port.protocol}`;
    const host = port.ip === null || port.ip === "0.0.0.0" ? "localhost" : port.ip;
    return `${host}:${port.public}${port.protocol === "tcp" ? "" : `/${port.protocol}`}`;
}

/** The URL a published TCP port serves on this Mac, if it serves one. */
export function portUrl(port: Port): string | null {
    if (port.public === null || port.protocol !== "tcp") return null;
    const host = port.ip === null || port.ip === "0.0.0.0" ? "localhost" : port.ip;
    return `http://${host}:${port.public}`;
}

export type Tone = "live" | "warn" | "danger" | "idle";

/** The colour a container's state reads in, with an unhealthy or restarting one warning. */
export function toneOf(container: Pick<Container, "state" | "status">): Tone {
    if (container.state === "running") return /\(unhealthy\)/.test(container.status) ? "warn" : "live";
    if (container.state === "restarting" || container.state === "paused") return "warn";
    if (container.state === "dead" || /^Exited \((?!0\))\d+\)/.test(container.status)) return "danger";
    return "idle";
}

export function isRunning(container: Pick<Container, "state">): boolean {
    return container.state === "running" || container.state === "restarting";
}

/** `docker.io/library/postgres:16` as people say it, `postgres:16`. */
export function shortImage(image: string): string {
    return image.replace(/^docker\.io\//, "").replace(/^library\//, "");
}

export function percent(value: number, of: number): number {
    if (of <= 0) return 0;
    return Math.min(100, Math.round((value / of) * 1000) / 10);
}
