import { createPluginBackend, type PluginStream, type PluginStreamHandlers } from "../../plugin-api/backend";
import { AZURE_PLUGIN } from "./kinds";

const backend = createPluginBackend(AZURE_PLUGIN.id);

export interface Subscription {
    id: string;
    name: string;
    tenantId: string;
    state: string;
    isDefault: boolean;
    user: string;
}

export interface AzureStatus {
    /** The CLI is installed and an account is signed in. */
    ready: boolean;
    configured: boolean;
    authFailed: boolean;
    /** cli-missing, signed-out or auth when not ready. */
    category: string | null;
    message: string | null;
    subscriptions: Subscription[];
    current: Subscription | null;
    user: string;
    cliVersion: string | null;
}

export interface Group {
    name: string;
    location: string;
    state: string;
    tags: [string, string][];
}

export interface Resource {
    id: string;
    name: string;
    /** The full type, such as microsoft.web/sites. */
    kind: string;
    /** What people call it, such as Web app. */
    label: string;
    group: string;
    location: string;
    state: string | null;
    sku: string | null;
    tags: [string, string][];
}

export interface KindCount {
    kind: string;
    label: string;
    count: number;
}

export interface Inventory {
    resources: Resource[];
    kinds: KindCount[];
    truncated: boolean;
}

export interface Machine {
    id: string;
    name: string;
    group: string;
    location: string;
    /** running, deallocated, stopped or starting. */
    power: string;
    size: string;
    os: string;
    image: string | null;
    privateIp: string | null;
    publicIp: string | null;
    zones: string[];
    tags: [string, string][];
}

export type MachineAction = "start" | "deallocate" | "powerOff" | "restart";

export interface App {
    id: string;
    name: string;
    group: string;
    location: string;
    state: string;
    kind: string;
    functions: boolean;
    host: string | null;
    httpsOnly: boolean;
    runtime: string | null;
    plan: string | null;
    tags: [string, string][];
}

export interface Setting {
    name: string;
    fromKeyVault: boolean;
}

export interface Slot {
    name: string;
    state: string;
    host: string | null;
}

export interface AppDetail extends App {
    slots: Slot[];
    /** Setting names only; no value ever leaves Azure. */
    settings: Setting[];
    alwaysOn: boolean;
    inboundIp: string | null;
    outboundIps: string[];
}

export type AppAction = "start" | "stop" | "restart";

export interface StorageAccount {
    id: string;
    name: string;
    group: string;
    location: string;
    sku: string;
    tier: string | null;
    kind: string;
    publicAccess: boolean;
    httpsOnly: boolean;
    created: string | null;
}

export interface Container {
    name: string;
    publicAccess: string;
    lastChanged: string | null;
    leased: boolean;
}

export interface Vault {
    id: string;
    name: string;
    group: string;
    location: string;
    uri: string | null;
    softDelete: boolean;
    purgeProtection: boolean;
    rbac: boolean;
}

export interface ActivityEvent {
    at: string;
    operation: string;
    status: string;
    level: string;
    resource: string;
    group: string;
    kind: string;
    who: string | null;
    message: string | null;
    correlation: string | null;
}

export interface Metric {
    name: string;
    unit: string;
    points: (number | null)[];
    latest: number | null;
    total: number | null;
}

/** What a call is for: a subscription, and sometimes one of its resource groups. */
export interface Scope {
    subscription?: string;
    group?: string;
}

export const azureApi = {
    status: () => backend.call<AzureStatus>("status"),
    signIn: (tenant?: string) => backend.call<AzureStatus>("signIn", { tenant }),
    useSubscription: (subscription: string) => backend.call<AzureStatus>("useSubscription", { subscription }),
    groups: (subscription: string) => backend.call<Group[]>("groups", { subscription }),
    resources: (scope: Scope, find: string) => backend.call<Inventory>("resources", { ...scope, find: find || undefined }),
    vms: (scope: Scope) => backend.call<Machine[]>("vms", scope),
    vm: (scope: Scope, name: string) => backend.call<Machine>("vm", { ...scope, name }),
    vmAct: (scope: Scope, name: string, action: MachineAction) => backend.call<{ message: string }>("vmAct", { ...scope, name, action }),
    apps: (scope: Scope) => backend.call<App[]>("apps", scope),
    app: (scope: Scope, name: string) => backend.call<AppDetail>("app", { ...scope, name }),
    appAct: (scope: Scope, name: string, action: AppAction) => backend.call<{ message: string }>("appAct", { ...scope, name, action }),
    tailAppLogs: (scope: Scope, name: string, handlers: PluginStreamHandlers<{ line: string }>): PluginStream =>
        backend.stream("tailAppLogs", { ...scope, name }, handlers),
    storage: (scope: Scope) => backend.call<StorageAccount[]>("storage", scope),
    containers: (scope: Scope, name: string) => backend.call<Container[]>("containers", { ...scope, name }),
    vaults: (scope: Scope) => backend.call<Vault[]>("vaults", scope),
    activity: (scope: Scope, hours: number, failedOnly: boolean) => backend.call<ActivityEvent[]>("activity", { ...scope, hours, failedOnly }),
    metrics: (id: string, names: string[], hours: number) => backend.call<Metric[]>("metrics", { id, names, hours }),
};
