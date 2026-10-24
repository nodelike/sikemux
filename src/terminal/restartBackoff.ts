export interface RestartBackoffOptions {
    readonly delaysMs?: readonly number[];
    readonly windowMs?: number;
    readonly maxRestartsPerWindow?: number;
}

export interface RestartBackoffDecision {
    readonly delayMs: number;
    readonly throttled: boolean;
}

const DEFAULT_DELAYS_MS = [100, 250, 500, 1_000, 2_000] as const;

/**
 * Bounds renderer-only PTY reattachments without delaying the PTY process.
 * The process owner stays alive while a failed renderer cools
 * down, so output can be recovered from the next bounded attach snapshot.
 */
export class RendererRestartBackoff {
    readonly #delaysMs: readonly number[];
    readonly #windowMs: number;
    readonly #maxRestartsPerWindow: number;
    readonly #restartTimes: number[] = [];
    #consecutiveRestarts = 0;

    constructor(options: RestartBackoffOptions = {}) {
        const delays = options.delaysMs ?? DEFAULT_DELAYS_MS;
        if (delays.length === 0 || delays.some((delay) => !Number.isFinite(delay) || delay < 0)) {
            throw new RangeError("restart delays must contain finite non-negative values");
        }
        this.#delaysMs = [...delays];
        this.#windowMs = options.windowMs ?? 10_000;
        this.#maxRestartsPerWindow = options.maxRestartsPerWindow ?? 4;
        if (!Number.isFinite(this.#windowMs) || this.#windowMs <= 0) throw new RangeError("restart window must be positive");
        if (!Number.isInteger(this.#maxRestartsPerWindow) || this.#maxRestartsPerWindow <= 0) {
            throw new RangeError("restart budget must be a positive integer");
        }
    }

    next(now: number): RestartBackoffDecision {
        if (!Number.isFinite(now)) throw new RangeError("restart timestamp must be finite");
        this.#prune(now);

        const baseDelay = this.#delaysMs[Math.min(this.#consecutiveRestarts, this.#delaysMs.length - 1)];
        this.#consecutiveRestarts += 1;

        let delayMs = baseDelay;
        let throttled = false;
        if (this.#restartTimes.length >= this.#maxRestartsPerWindow) {
            const windowDelay = this.#windowMs - (now - this.#restartTimes[0]);
            delayMs = Math.max(delayMs, windowDelay);
            throttled = true;
        }
        this.#restartTimes.push(now);
        return { delayMs: Math.max(0, Math.ceil(delayMs)), throttled };
    }

    reset(): void {
        this.#restartTimes.length = 0;
        this.#consecutiveRestarts = 0;
    }

    #prune(now: number): void {
        const cutoff = now - this.#windowMs;
        while (this.#restartTimes.length > 0 && this.#restartTimes[0] <= cutoff) this.#restartTimes.shift();
        if (this.#restartTimes.length === 0) this.#consecutiveRestarts = 0;
    }
}
