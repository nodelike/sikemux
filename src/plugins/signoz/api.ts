import { createPluginBackend, isPluginFailure } from "../../plugin-api/backend";
import { invalidate } from "../../plugin-api/resources";
import { SIGNOZ_PLUGIN_ID } from "./kinds";

const backend = createPluginBackend(SIGNOZ_PLUGIN_ID);

export type AuthMode = "session" | "apiKey";

export interface SignozStatus {
    configured: boolean;
    url: string;
    auth: AuthMode;
    email: string;
    keyFromEnvironment: boolean;
    version: string | null;
    ok: boolean;
    authFailed: boolean;
    message: string | null;
}

export interface SsoProvider {
    provider: string;
    url: string;
}

export interface OrgSignIn {
    id: string;
    name: string;
    password: boolean;
    sso: SsoProvider[];
}

export interface Inspection {
    url: string;
    version: string | null;
    accountExists: boolean | null;
    orgs: OrgSignIn[];
}

export type FilterOp = "equals" | "not-equals" | "contains" | "not-contains" | "exists" | "not-exists";

export interface Filter {
    key: string;
    op: FilterOp;
    value: string;
}

/** What every query narrows by. A fixed `start`/`end` wins over `minutes`. */
export interface Scope {
    service?: string;
    environment?: string;
    filters?: Filter[];
    expression?: string;
    start?: number;
    end?: number;
    minutes?: number;
}

export interface LogLine {
    id: string;
    timestamp: string;
    service: string | null;
    severity: string | null;
    body: string;
    traceId: string | null;
    spanId: string | null;
    attributes: Record<string, unknown>;
    resources: Record<string, unknown>;
}

export interface LogSearch extends Scope {
    text?: string;
    severities?: string[];
    traceId?: string;
    limit?: number;
    offset?: number;
}

export interface LogPage {
    lines: LogLine[];
    nextOffset: number | null;
}

export interface TailTick {
    lines: LogLine[];
    error: string | null;
}

export interface ServiceHealth {
    service: string;
    environment: string | null;
    calls: number;
    errors: number;
    errorRate: number;
    p99Ms: number;
}

export type Point = [number, number];

export interface ServiceOverview {
    calls: number;
    errors: number;
    errorRate: number;
    perMinute: number;
    p99Ms: number;
    p50Ms: number;
    requests: Point[];
    failures: Point[];
    p99: Point[];
}

export interface Operation {
    name: string;
    calls: number;
    errors: number;
    errorRate: number;
    p99Ms: number;
    p50Ms: number;
}

export interface ErrorGroup {
    pattern: string;
    sample: string;
    count: number;
    firstSeen: number;
    lastSeen: number;
}

export type TraceOrder = "slowest" | "recent";

export interface TraceSearch extends Scope {
    errorsOnly?: boolean;
    minDurationMs?: number;
    order?: TraceOrder;
    limit?: number;
    offset?: number;
}

export interface TraceSummary {
    traceId: string;
    timestamp: string;
    service: string;
    name: string;
    durationMs: number;
    error: boolean;
    statusCode: string | null;
}

export interface TracePage {
    traces: TraceSummary[];
    nextOffset: number | null;
}

export interface TraceSpan {
    spanId: string;
    parentId: string | null;
    name: string;
    service: string;
    depth: number;
    offsetMs: number;
    durationMs: number;
    error: boolean;
    status: string | null;
    kind: string | null;
}

export interface Trace {
    traceId: string;
    start: string;
    durationMs: number;
    errorCount: number;
    services: string[];
    spans: TraceSpan[];
    truncated: boolean;
}

export type Signal = "logs" | "traces";

export interface FieldKey {
    name: string;
    context: string;
    dataType: string;
}

export interface VolumeBucket {
    start: number;
    counts: Record<string, number>;
}

export interface DashboardSummary {
    id: string;
    title: string;
    description: string;
    tags: string[];
    panels: number;
}

export interface DashboardVariable {
    name: string;
    options: string[];
    selected: string;
}

export interface PanelLayout {
    x: number;
    y: number;
    w: number;
    h: number;
}

export interface DashboardPanel {
    id: string;
    title: string;
    kind: string;
    unit: string;
    layout: PanelLayout;
    drawable: boolean;
    query: unknown;
}

export interface Dashboard {
    id: string;
    title: string;
    variables: DashboardVariable[];
    panels: DashboardPanel[];
}

export interface Series {
    label: string;
    points: Point[];
}

export type PanelData =
    | { shape: "series"; series: Series[] }
    | { shape: "table"; columns: { name: string; aggregation: boolean }[]; rows: unknown[][] }
    | { shape: "value"; value: number | null };

export interface PanelRequest extends Scope {
    kind: string;
    query: unknown;
    variables: Record<string, string>;
}

export function failureMessage(error: unknown): string {
    return isPluginFailure(error) ? error.message : String(error);
}

function isSignedOut(error: unknown): boolean {
    if (!isPluginFailure(error)) return false;
    return (
        error.category === "auth" ||
        error.category === "unconfigured" ||
        (error.category === "http" && (error.status === 401 || error.status === 403))
    );
}

/** A lapsed session makes every cached answer stale, so the next read lands on the sign-in form. */
async function read<T>(method: string, params?: unknown): Promise<T> {
    try {
        return await backend.call<T>(method, params);
    } catch (error) {
        if (isSignedOut(error)) invalidate((kind) => kind.startsWith("signoz."));
        throw error;
    }
}

export const signozApi = {
    status: () => backend.call<SignozStatus>("status"),
    inspect: (url: string, email?: string) => backend.call<Inspection>("inspect", { url, email }),
    signIn: (url: string, email: string, password: string, orgId?: string) => backend.call<SignozStatus>("signIn", { url, email, password, orgId }),
    useApiKey: (url: string, apiKey?: string, account?: string) => backend.call<SignozStatus>("useApiKey", { url, apiKey, account }),
    signOut: () => backend.call<void>("signOut"),

    services: (scope: Scope) => read<ServiceHealth[]>("services", scope),
    serviceOverview: (scope: Scope) => read<ServiceOverview>("serviceOverview", scope),
    operations: (scope: Scope) => read<Operation[]>("operations", scope),
    errorGroups: (scope: Scope) => read<ErrorGroup[]>("errorGroups", scope),
    searchLogs: (search: LogSearch) => read<LogPage>("searchLogs", search),
    searchTraces: (search: TraceSearch) => read<TracePage>("searchTraces", search),
    trace: (traceId: string) => read<Trace>("trace", { traceId }),
    logVolume: (search: LogSearch & { buckets?: number }) => read<VolumeBucket[]>("logVolume", search),
    dashboards: () => read<DashboardSummary[]>("dashboards"),
    dashboard: (id: string) => read<Dashboard>("dashboard", { id }),
    panel: (request: PanelRequest) => read<PanelData>("panel", request),
    fieldKeys: (signal: Signal, search: string) => read<FieldKey[]>("fieldKeys", { signal, search }),
    fieldValues: (signal: Signal, name: string, search: string) => read<string[]>("fieldValues", { signal, name, search }),

    tailStart: (search: LogSearch, onTick: (tick: TailTick) => void) => backend.openStream<TailTick>("tailLogs", search, onTick),
    tailStop: (id: number) => backend.closeStream(id),
};
