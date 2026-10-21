import type { ActivityEvent, App, AppDetail, AzureStatus, Group, Inventory, Machine, StorageAccount } from "./api";

export const signedIn: AzureStatus = {
    ready: true,
    configured: true,
    authFailed: false,
    category: null,
    message: null,
    user: "ankit@acme.dev",
    cliVersion: "2.90.0",
    subscriptions: [
        { id: "sub-1", name: "Acme production", tenantId: "t1", state: "Enabled", isDefault: true, user: "ankit@acme.dev" },
        { id: "sub-2", name: "Acme sandbox", tenantId: "t1", state: "Enabled", isDefault: false, user: "ankit@acme.dev" },
    ],
    current: { id: "sub-1", name: "Acme production", tenantId: "t1", state: "Enabled", isDefault: true, user: "ankit@acme.dev" },
};

export const noCli: AzureStatus = {
    ...signedIn,
    ready: false,
    configured: false,
    category: "cli-missing",
    message: "azure: the Azure CLI isn't installed. Install it (brew install azure-cli), then sign in with az login.",
    subscriptions: [],
    current: null,
};

export const signedOut: AzureStatus = { ...noCli, category: "signed-out", message: "azure: not signed in; run az login" };

export const GROUPS: Group[] = [
    { name: "acme-prod", location: "uaenorth", state: "Succeeded", tags: [] },
    { name: "acme-staging", location: "uaenorth", state: "Succeeded", tags: [] },
];

export const RUNNING: Machine = {
    id: "/subscriptions/sub-1/resourceGroups/acme-prod/providers/Microsoft.Compute/virtualMachines/orders-worker-1",
    name: "orders-worker-1",
    group: "acme-prod",
    location: "uaenorth",
    power: "running",
    size: "Standard_D2s_v5",
    os: "Linux",
    image: "ubuntu-24_04-lts server",
    privateIp: "10.0.1.4",
    publicIp: "20.74.12.8",
    zones: ["1"],
    tags: [["env", "prod"]],
};

export const STOPPED: Machine = {
    ...RUNNING,
    id: "/subscriptions/sub-1/resourceGroups/acme-staging/providers/Microsoft.Compute/virtualMachines/staging-box",
    name: "staging-box",
    group: "acme-staging",
    power: "deallocated",
    publicIp: null,
    zones: [],
};

export const WEB_APP: App = {
    id: "/subscriptions/sub-1/resourceGroups/acme-prod/providers/Microsoft.Web/sites/acme-storefront",
    name: "acme-storefront",
    group: "acme-prod",
    location: "uaenorth",
    state: "Running",
    kind: "app,linux",
    functions: false,
    host: "acme-storefront.azurewebsites.net",
    httpsOnly: true,
    runtime: "NODE|22-lts",
    plan: "acme-prod-plan",
    tags: [],
};

export const WEB_APP_DETAIL: AppDetail = {
    ...WEB_APP,
    alwaysOn: true,
    inboundIp: "20.74.12.40",
    outboundIps: ["20.74.12.41"],
    slots: [{ name: "staging", state: "Running", host: "acme-storefront-staging.azurewebsites.net" }],
    settings: [
        { name: "DATABASE_URL", fromKeyVault: true },
        { name: "NODE_ENV", fromKeyVault: false },
    ],
};

export const ACCOUNT: StorageAccount = {
    id: "/subscriptions/sub-1/resourceGroups/acme-prod/providers/Microsoft.Storage/storageAccounts/acmeprodassets",
    name: "acmeprodassets",
    group: "acme-prod",
    location: "uaenorth",
    sku: "Standard_ZRS",
    tier: "Hot",
    kind: "StorageV2",
    publicAccess: true,
    httpsOnly: true,
    created: null,
};

export const FAILED_EVENT: ActivityEvent = {
    at: "2026-10-19T12:00:00Z",
    operation: "Create or Update Virtual Machine",
    status: "Failed",
    level: "Error",
    resource: "build-agent",
    group: "shared",
    kind: "microsoft.compute/virtualmachines",
    who: "ankit@acme.dev",
    message: "Operation could not be completed as it results in exceeding approved quota.",
    correlation: "c-3",
};

export const OK_EVENT: ActivityEvent = {
    ...FAILED_EVENT,
    operation: "Restart Web App",
    status: "Succeeded",
    level: "Informational",
    resource: "acme-storefront",
    group: "acme-prod",
    message: null,
};

export const INVENTORY: Inventory = {
    resources: [
        {
            id: RUNNING.id,
            name: RUNNING.name,
            kind: "microsoft.compute/virtualmachines",
            label: "Virtual machine",
            group: "acme-prod",
            location: "uaenorth",
            state: "running",
            sku: "Standard_D2s_v5",
            tags: [],
        },
        {
            id: WEB_APP.id,
            name: WEB_APP.name,
            kind: "microsoft.web/sites",
            label: "Web app",
            group: "acme-prod",
            location: "uaenorth",
            state: "Running",
            sku: null,
            tags: [],
        },
        {
            id: ACCOUNT.id,
            name: ACCOUNT.name,
            kind: "microsoft.storage/storageaccounts",
            label: "Storage account",
            group: "acme-prod",
            location: "uaenorth",
            state: "Succeeded",
            sku: "Standard_ZRS",
            tags: [],
        },
    ],
    kinds: [
        { kind: "microsoft.compute/virtualmachines", label: "Virtual machine", count: 1 },
        { kind: "microsoft.web/sites", label: "Web app", count: 1 },
        { kind: "microsoft.storage/storageaccounts", label: "Storage account", count: 1 },
    ],
    truncated: false,
};
