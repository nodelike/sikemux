/* Terminals saved in the layout may still be running in the core. Each can be
   taken over once per page, by the pane that showed it, before anything in
   this page has spawned or killed a terminal of the same id. */
const resumable = new Set<number>();
const spawnedHere = new Set<number>();

export function offerResumableSessions(ids: Iterable<number>): void {
    for (const id of ids) resumable.add(id);
}

export function isResumableSession(id: number | undefined): boolean {
    return id !== undefined && resumable.has(id);
}

export function takeResumableSession(id: number): boolean {
    return resumable.delete(id);
}

/** Terminals this page started are its own, whatever the saved layout says. */
export function noteSpawnedSession(id: number): void {
    spawnedHere.add(id);
}

export function spawnedThisPage(id: number): boolean {
    return spawnedHere.has(id);
}
