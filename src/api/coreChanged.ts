import { getIpcTransport } from "./transport";
import type { CoreChangeTopic } from "@shared/wire.generated";

/** What the core says changed: read `topic` again, for `project` or every project, and only `paths` inside it when given. */
export interface CoreChange {
    readonly topic: CoreChangeTopic;
    readonly project: string | null;
    readonly paths: readonly string[] | null;
    /** The other host whose core said so, or none for this one. */
    readonly host?: string;
}

type Listener = (change: CoreChange) => void;

const listeners = new Map<CoreChangeTopic, Set<Listener>>();
let subscribed = false;

function subscribe(): void {
    if (subscribed) return;
    subscribed = true;
    void getIpcTransport()
        .subscribe<CoreChange>("core_changed", ({ payload }) => {
            for (const listener of [...(listeners.get(payload.topic) ?? [])]) listener(payload);
        })
        .catch(() => {
            subscribed = false;
        });
}

/** Calls `listener` each time the core says `topic` changed. */
export function onCoreChanged(topic: CoreChangeTopic, listener: Listener): () => void {
    subscribe();
    const set = listeners.get(topic) ?? new Set<Listener>();
    set.add(listener);
    listeners.set(topic, set);
    return () => {
        set.delete(listener);
        if (!set.size) listeners.delete(topic);
    };
}
