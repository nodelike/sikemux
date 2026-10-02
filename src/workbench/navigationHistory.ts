export interface NavigationLocationInput {
    readonly project: string;
    readonly path: string;
    /** Zero-based document line. */
    readonly line?: number;
    /** Zero-based document column; only valid when `line` is present. */
    readonly column?: number;
    readonly symbol?: string;
}

/** A content-free, immutable target suitable for editor navigation. */
export interface NavigationLocation extends NavigationLocationInput {
    readonly project: string;
    readonly path: string;
}

export type NavigationHistoryPushResult = "pushed" | "duplicate" | "invalid" | "stale";

export type NavigationLocationFreshness = (location: NavigationLocation) => boolean;

export interface NavigationHistoryOptions {
    /** Total retained locations, including the current location. */
    readonly capacity?: number;
    /** Return false when a project/path can no longer be navigated to. */
    readonly isLocationCurrent?: NavigationLocationFreshness;
}

export interface NavigationHistorySnapshot {
    readonly capacity: number;
    readonly size: number;
    readonly current: NavigationLocation | null;
    /** Oldest to nearest; the nearest back target is the final element. */
    readonly backward: readonly NavigationLocation[];
    /** Farthest to nearest; the nearest forward target is the final element. */
    readonly forward: readonly NavigationLocation[];
    readonly canGoBack: boolean;
    readonly canGoForward: boolean;
}

export const NAVIGATION_HISTORY_LIMITS = Object.freeze({
    defaultCapacity: 128,
    maxCapacity: 4_096,
    maxProjectBytes: 4_096,
    maxPathBytes: 4_096,
    maxSymbolBytes: 256,
    maxPosition: 0xffff_ffff,
});

const LOCATION_KEYS = new Set<PropertyKey>(["project", "path", "line", "column", "symbol"]);
const UTF8_ENCODER = new TextEncoder();

function containsControlCharacter(value: string): boolean {
    for (let index = 0; index < value.length; index += 1) {
        const code = value.charCodeAt(index);
        if (code <= 31 || (code >= 127 && code <= 159)) return true;
    }
    return false;
}

function isBoundedText(value: unknown, maxBytes: number, trimmed: boolean): value is string {
    return (
        typeof value === "string" &&
        value.length > 0 &&
        value.length <= maxBytes &&
        value.trim().length > 0 &&
        (!trimmed || value === value.trim()) &&
        !containsControlCharacter(value) &&
        UTF8_ENCODER.encode(value).byteLength <= maxBytes
    );
}

function isPosition(value: unknown): value is number {
    return Number.isSafeInteger(value) && (value as number) >= 0 && (value as number) <= NAVIGATION_HISTORY_LIMITS.maxPosition;
}

function dataProperties(value: unknown): Readonly<Record<string, unknown>> | null {
    if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
    try {
        const properties = Object.getOwnPropertyDescriptors(value);
        for (const key of Reflect.ownKeys(properties)) {
            if (!LOCATION_KEYS.has(key)) return null;
            const descriptor = properties[key as keyof typeof properties];
            if (!descriptor || !("value" in descriptor)) return null;
        }
        return Object.fromEntries(Object.entries(properties).map(([key, descriptor]) => [key, descriptor.value]));
    } catch {
        return null;
    }
}

/** Strictly validates, clones, and freezes an untrusted navigation target. */
export function parseNavigationLocation(value: unknown): NavigationLocation | null {
    const properties = dataProperties(value);
    if (!properties) return null;
    const project = properties.project;
    const path = properties.path;
    const line = properties.line;
    const column = properties.column;
    const symbol = properties.symbol;
    if (
        !isBoundedText(project, NAVIGATION_HISTORY_LIMITS.maxProjectBytes, false) ||
        !isBoundedText(path, NAVIGATION_HISTORY_LIMITS.maxPathBytes, false) ||
        (line !== undefined && !isPosition(line)) ||
        (column !== undefined && (!isPosition(column) || line === undefined)) ||
        (symbol !== undefined && !isBoundedText(symbol, NAVIGATION_HISTORY_LIMITS.maxSymbolBytes, true))
    ) {
        return null;
    }

    return Object.freeze({
        project,
        path,
        ...(line === undefined ? {} : { line }),
        ...(column === undefined ? {} : { column }),
        ...(symbol === undefined ? {} : { symbol }),
    });
}

function sameLocation(left: NavigationLocation, right: NavigationLocation): boolean {
    return (
        left.project === right.project &&
        left.path === right.path &&
        left.line === right.line &&
        left.column === right.column &&
        left.symbol === right.symbol
    );
}

function requireCapacity(value: number): number {
    if (!Number.isSafeInteger(value) || value <= 0 || value > NAVIGATION_HISTORY_LIMITS.maxCapacity) {
        throw new RangeError(`navigation history capacity must be between 1 and ${NAVIGATION_HISTORY_LIMITS.maxCapacity}`);
    }
    return value;
}

/** Browser-style, renderer-independent navigation history. */
export class NavigationHistory {
    readonly capacity: number;
    private readonly isLocationCurrent: NavigationLocationFreshness;
    private readonly backwardStack: NavigationLocation[] = [];
    private readonly forwardStack: NavigationLocation[] = [];
    private current: NavigationLocation | null = null;

    constructor(options: NavigationHistoryOptions = {}) {
        this.capacity = requireCapacity(options.capacity ?? NAVIGATION_HISTORY_LIMITS.defaultCapacity);
        this.isLocationCurrent = options.isLocationCurrent ?? (() => true);
    }

    push(value: NavigationLocationInput): NavigationHistoryPushResult;
    push(value: unknown): NavigationHistoryPushResult {
        const location = parseNavigationLocation(value);
        if (!location) return "invalid";
        if (!this.isCurrent(location)) return "stale";
        if (this.current && sameLocation(this.current, location)) return "duplicate";

        if (this.current && this.isCurrent(this.current)) this.backwardStack.push(this.current);
        this.current = location;
        this.forwardStack.length = 0;
        while (this.size > this.capacity) this.backwardStack.shift();
        return "pushed";
    }

    back(): NavigationLocation | null {
        return this.navigate(this.backwardStack, this.forwardStack);
    }

    forward(): NavigationLocation | null {
        return this.navigate(this.forwardStack, this.backwardStack);
    }

    getSnapshot(): NavigationHistorySnapshot {
        return Object.freeze({
            capacity: this.capacity,
            size: this.size,
            current: this.current,
            backward: Object.freeze(this.backwardStack.slice()),
            forward: Object.freeze(this.forwardStack.slice()),
            canGoBack: this.backwardStack.length > 0,
            canGoForward: this.forwardStack.length > 0,
        });
    }

    private get size(): number {
        return this.backwardStack.length + this.forwardStack.length + (this.current ? 1 : 0);
    }

    private navigate(source: NavigationLocation[], destination: NavigationLocation[]): NavigationLocation | null {
        let target: NavigationLocation | undefined;
        while ((target = source.pop())) {
            if (this.isCurrent(target)) break;
        }
        if (!target) return null;

        if (this.current && this.isCurrent(this.current)) destination.push(this.current);
        this.current = target;
        return target;
    }

    private isCurrent(location: NavigationLocation): boolean {
        try {
            return this.isLocationCurrent(location) === true;
        } catch {
            return false;
        }
    }
}
