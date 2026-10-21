import type { ActivityEvent, Machine } from "./api";

export type Tone = "live" | "warn" | "danger" | "idle";

/** The colour a machine's power state reads in. */
export function powerTone(power: string): Tone {
    const state = power.toLowerCase();
    if (state === "running") return "live";
    if (state === "deallocated" || state === "stopped") return "idle";
    if (state.includes("ing")) return "warn";
    return "warn";
}

export function appTone(state: string): Tone {
    return state === "Running" ? "live" : state === "Stopped" ? "idle" : "warn";
}

export function eventTone(event: Pick<ActivityEvent, "status" | "level">): Tone {
    if (event.level === "Error" || event.level === "Critical" || event.status === "Failed") return "danger";
    if (event.level === "Warning") return "warn";
    if (event.status === "Succeeded") return "live";
    return "idle";
}

/** How long ago an ISO time was: 45s, 12m, 3h, 6d, 4mo. */
export function ago(at: string | null, now = Date.now()): string {
    if (!at) return "—";
    const seconds = Math.max(0, Math.floor((now - Date.parse(at)) / 1000));
    if (Number.isNaN(seconds)) return "—";
    if (seconds < 60) return `${seconds}s`;
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
    if (seconds < 86_400) return `${Math.floor(seconds / 3600)}h`;
    if (seconds < 86_400 * 60) return `${Math.floor(seconds / 86_400)}d`;
    return `${Math.floor(seconds / (86_400 * 30))}mo`;
}

/** An Azure region as people say it: uaenorth becomes UAE North. */
const REGION_WORDS = [
    "north",
    "south",
    "east",
    "west",
    "central",
    "europe",
    "india",
    "asia",
    "us",
    "uk",
    "uae",
    "australia",
    "japan",
    "korea",
    "canada",
    "brazil",
    "france",
    "germany",
    "norway",
    "sweden",
    "switzerland",
    "poland",
    "italy",
    "israel",
    "qatar",
    "africa",
    "china",
];

export function region(location: string): string {
    if (!location) return "—";
    let rest = location.toLowerCase();
    const words: string[] = [];
    while (rest.length > 0) {
        const found = REGION_WORDS.find((word) => rest.startsWith(word));
        if (!found) break;
        words.push(found === "us" || found === "uk" || found === "uae" ? found.toUpperCase() : found[0].toUpperCase() + found.slice(1));
        rest = rest.slice(found.length);
    }
    if (rest.length > 0) words.push(rest);
    return words.join(" ") || location;
}

/** A machine's size without its family prefix, as the portal shows it. */
export function size(value: string): string {
    return value.replace(/^Standard_/, "");
}

/** What a VM power state can be asked to do next. */
export function actionsFor(
    machine: Pick<Machine, "power">,
): { action: "start" | "deallocate" | "powerOff" | "restart"; label: string; danger?: boolean }[] {
    const running = machine.power.toLowerCase() === "running";
    if (running) {
        return [
            { action: "restart", label: "Restart" },
            { action: "powerOff", label: "Stop" },
            { action: "deallocate", label: "Stop and deallocate", danger: true },
        ];
    }
    return [{ action: "start", label: "Start" }];
}
