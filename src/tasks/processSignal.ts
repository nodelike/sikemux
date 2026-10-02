const listeners = new Set<() => void>();

/** Called whenever a task's process starts or ends. */
export function onTaskProcessChange(listener: () => void): () => void {
    listeners.add(listener);
    return () => {
        listeners.delete(listener);
    };
}

export function taskProcessChanged(): void {
    for (const listener of [...listeners]) listener();
}
