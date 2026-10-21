import { collectPanes } from "../state/layout";
import { useStore, type StoreState } from "../state/store";

interface HeldResource {
    readonly fingerprint: string;
    readonly value: unknown;
    readonly dispose: () => void;
}

const held = new Map<string, HeldResource>();

/** Keeps a pane's resource across remounts, until the pane closes or the fingerprint changes. */
export function paneResource<Value>(paneId: string, fingerprint: string, create: () => { value: Value; dispose: () => void }): Value {
    const existing = held.get(paneId);
    if (existing?.fingerprint === fingerprint) return existing.value as Value;
    existing?.dispose();
    const created = create();
    held.set(paneId, { fingerprint, value: created.value, dispose: created.dispose });
    return created.value;
}

function releaseMissing(windows: StoreState["windows"]): void {
    const live = new Set<string>();
    for (const window of Object.values(windows)) for (const pane of collectPanes(window.root)) live.add(pane.id);
    for (const [paneId, resource] of held) {
        if (live.has(paneId)) continue;
        held.delete(paneId);
        resource.dispose();
    }
}

export function releaseClosedPaneResources(): () => void {
    return useStore.subscribe((state, previous) => {
        if (held.size > 0 && state.windows !== previous.windows) releaseMissing(state.windows);
    });
}

export function paneResourceCount(): number {
    return held.size;
}

export function resetPaneResourcesForTests(): void {
    for (const resource of held.values()) resource.dispose();
    held.clear();
}
