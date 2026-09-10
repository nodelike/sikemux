export interface HarnessEvent {
    cursor: string;
    project: string;
    kind: string;
    executionId?: string;
}

export class HarnessEvents {
    private readonly epoch = crypto.randomUUID();
    private sequence = 0;
    private readonly events: HarnessEvent[] = [];
    private readonly listeners = new Set<() => void>();

    get cursor(): string {
        return `${this.epoch}:${this.sequence}`;
    }

    publish(event: Omit<HarnessEvent, "cursor">): void {
        this.sequence++;
        this.events.push({ ...event, cursor: this.cursor });
        if (this.events.length > 256) this.events.shift();
        for (const listener of [...this.listeners]) listener();
    }

    async wait(project: string, after: string, timeoutMs: number, executionId?: string, signal?: AbortSignal) {
        const [epoch, raw] = after.split(":");
        const sequence = Number(raw);
        if (
            after !== `${epoch}:${raw}` ||
            epoch !== this.epoch ||
            !/^\d+$/.test(raw ?? "") ||
            !Number.isSafeInteger(sequence) ||
            sequence > this.sequence
        ) {
            throw new Error("Event cursor is invalid or belongs to an earlier app session; inspect the workspace again");
        }
        if (!Number.isInteger(timeoutMs) || timeoutMs < 0 || timeoutMs > 30_000) throw new Error("timeoutMs must be between 0 and 30000");
        if (this.listeners.size >= 32) throw new Error("Too many event waits");
        const snapshot = () => ({
            events: this.events
                .filter(
                    (event) =>
                        Number(event.cursor.split(":")[1]) > sequence &&
                        event.project === project &&
                        (!executionId || event.executionId === executionId),
                )
                .map((event) => ({ cursor: event.cursor, kind: event.kind, executionId: event.executionId })),
            cursor: this.cursor,
            truncated: sequence < this.sequence - this.events.length,
        });
        const current = snapshot();
        if (current.events.length || current.truncated || timeoutMs === 0) return current;
        return new Promise<ReturnType<typeof snapshot>>((resolve, reject) => {
            if (signal?.aborted) {
                reject(signal.reason);
                return;
            }
            const cleanup = () => {
                clearTimeout(timer);
                this.listeners.delete(changed);
                signal?.removeEventListener("abort", aborted);
            };
            const changed = () => {
                const result = snapshot();
                if (result.events.length || result.truncated) {
                    cleanup();
                    resolve(result);
                }
            };
            const aborted = () => {
                cleanup();
                reject(signal?.reason);
            };
            const timer = setTimeout(() => {
                cleanup();
                resolve(snapshot());
            }, timeoutMs);
            this.listeners.add(changed);
            signal?.addEventListener("abort", aborted, { once: true });
        });
    }
}
