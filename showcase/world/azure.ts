type Params = Record<string, unknown>;

const ago = (minutes: number) =>
  new Date(Date.now() - minutes * 60_000).toISOString();

const SUBSCRIPTION = "816d8abc-dea6-482b-a948-11fabddb7a69";

const GROUPS = [
  {
    name: "acme-prod",
    location: "uaenorth",
    state: "Succeeded",
    tags: [["env", "prod"]],
  },
  {
    name: "acme-staging",
    location: "uaenorth",
    state: "Succeeded",
    tags: [["env", "staging"]],
  },
  { name: "shared", location: "centralindia", state: "Succeeded", tags: [] },
];

const id = (group: string, provider: string, name: string) =>
  `/subscriptions/${SUBSCRIPTION}/resourceGroups/${group}/providers/${provider}/${name}`;

const MACHINES = [
  {
    id: id("acme-prod", "Microsoft.Compute/virtualMachines", "orders-worker-1"),
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
    tags: [["env", "prod"]] as [string, string][],
  },
  {
    id: id("acme-prod", "Microsoft.Compute/virtualMachines", "orders-worker-2"),
    name: "orders-worker-2",
    group: "acme-prod",
    location: "uaenorth",
    power: "running",
    size: "Standard_D2s_v5",
    os: "Linux",
    image: "ubuntu-24_04-lts server",
    privateIp: "10.0.1.5",
    publicIp: null,
    zones: ["2"],
    tags: [["env", "prod"]] as [string, string][],
  },
  {
    id: id("acme-staging", "Microsoft.Compute/virtualMachines", "staging-box"),
    name: "staging-box",
    group: "acme-staging",
    location: "uaenorth",
    power: "deallocated",
    size: "Standard_B2s",
    os: "Linux",
    image: "debian-12 bookworm",
    privateIp: "10.1.0.4",
    publicIp: null,
    zones: [],
    tags: [["env", "staging"]] as [string, string][],
  },
  {
    id: id("shared", "Microsoft.Compute/virtualMachines", "build-agent"),
    name: "build-agent",
    group: "shared",
    location: "centralindia",
    power: "starting",
    size: "Standard_D4s_v5",
    os: "Windows",
    image: "WindowsServer 2022-datacenter",
    privateIp: "10.2.0.4",
    publicIp: null,
    zones: [],
    tags: [] as [string, string][],
  },
];

const APPS = [
  {
    id: id("acme-prod", "Microsoft.Web/sites", "acme-storefront"),
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
    tags: [["env", "prod"]] as [string, string][],
  },
  {
    id: id("acme-prod", "Microsoft.Web/sites", "acme-checkout-fn"),
    name: "acme-checkout-fn",
    group: "acme-prod",
    location: "uaenorth",
    state: "Running",
    kind: "functionapp,linux",
    functions: true,
    host: "acme-checkout-fn.azurewebsites.net",
    httpsOnly: true,
    runtime: "DOTNET-ISOLATED|8.0",
    plan: "acme-prod-plan",
    tags: [["env", "prod"]] as [string, string][],
  },
  {
    id: id("acme-staging", "Microsoft.Web/sites", "acme-storefront-staging"),
    name: "acme-storefront-staging",
    group: "acme-staging",
    location: "uaenorth",
    state: "Stopped",
    kind: "app,linux",
    functions: false,
    host: "acme-storefront-staging.azurewebsites.net",
    httpsOnly: true,
    runtime: "NODE|22-lts",
    plan: "acme-staging-plan",
    tags: [["env", "staging"]] as [string, string][],
  },
];

const STORAGE = [
  {
    id: id("acme-prod", "Microsoft.Storage/storageAccounts", "acmeprodassets"),
    name: "acmeprodassets",
    group: "acme-prod",
    location: "uaenorth",
    sku: "Standard_ZRS",
    tier: "Hot",
    kind: "StorageV2",
    publicAccess: true,
    httpsOnly: true,
    created: ago(60 * 24 * 300),
  },
  {
    id: id("acme-prod", "Microsoft.Storage/storageAccounts", "acmeprodbackups"),
    name: "acmeprodbackups",
    group: "acme-prod",
    location: "uaenorth",
    sku: "Standard_GRS",
    tier: "Cool",
    kind: "StorageV2",
    publicAccess: false,
    httpsOnly: true,
    created: ago(60 * 24 * 300),
  },
];

const CONTAINERS: Record<
  string,
  {
    name: string;
    publicAccess: string;
    lastChanged: string | null;
    leased: boolean;
  }[]
> = {
  acmeprodassets: [
    {
      name: "product-images",
      publicAccess: "Blob",
      lastChanged: ago(40),
      leased: false,
    },
    {
      name: "invoices",
      publicAccess: "None",
      lastChanged: ago(60 * 9),
      leased: false,
    },
  ],
  acmeprodbackups: [
    {
      name: "nightly",
      publicAccess: "None",
      lastChanged: ago(60 * 7),
      leased: true,
    },
  ],
};

const VAULTS = [
  {
    id: id("acme-prod", "Microsoft.KeyVault/vaults", "acme-prod-kv"),
    name: "acme-prod-kv",
    group: "acme-prod",
    location: "uaenorth",
    uri: "https://acme-prod-kv.vault.azure.net/",
    softDelete: true,
    purgeProtection: true,
    rbac: true,
  },
];

const ACTIVITY = [
  {
    at: ago(6),
    operation: "Restart Web App",
    status: "Succeeded",
    level: "Informational",
    resource: "acme-storefront",
    group: "acme-prod",
    kind: "microsoft.web/sites",
    who: "edon@acme.dev",
    message: null,
    correlation: "c-1",
  },
  {
    at: ago(22),
    operation: "Deallocate Virtual Machine",
    status: "Succeeded",
    level: "Informational",
    resource: "staging-box",
    group: "acme-staging",
    kind: "microsoft.compute/virtualmachines",
    who: "maya@acme.dev",
    message: null,
    correlation: "c-2",
  },
  {
    at: ago(48),
    operation: "Create or Update Virtual Machine",
    status: "Failed",
    level: "Error",
    resource: "build-agent",
    group: "shared",
    kind: "microsoft.compute/virtualmachines",
    who: "edon@acme.dev",
    message:
      "Operation could not be completed as it results in exceeding approved standardDSv5Family Cores quota.",
    correlation: "c-3",
  },
  {
    at: ago(140),
    operation: "Update Storage Account",
    status: "Succeeded",
    level: "Warning",
    resource: "acmeprodassets",
    group: "acme-prod",
    kind: "microsoft.storage/storageaccounts",
    who: "edon@acme.dev",
    message: "Blob public access was turned on.",
    correlation: "c-4",
  },
];

const EXTRA = [
  {
    name: "acme-prod-plan",
    kind: "microsoft.web/serverfarms",
    label: "App Service plan",
    group: "acme-prod",
    sku: "P1v3",
    state: "Succeeded",
  },
  {
    name: "acme-staging-plan",
    kind: "microsoft.web/serverfarms",
    label: "App Service plan",
    group: "acme-staging",
    sku: "B1",
    state: "Succeeded",
  },
  {
    name: "acme-prod-vnet",
    kind: "microsoft.network/virtualnetworks",
    label: "Virtual network",
    group: "acme-prod",
    sku: null,
    state: "Succeeded",
  },
  {
    name: "acme-orders-db",
    kind: "microsoft.dbforpostgresql/flexibleservers",
    label: "Postgres server",
    group: "acme-prod",
    sku: "Standard_D2ds_v5",
    state: "Ready",
  },
  {
    name: "acme-prod-redis",
    kind: "microsoft.cache/redis",
    label: "Redis cache",
    group: "acme-prod",
    sku: "Standard C1",
    state: "Succeeded",
  },
  {
    name: "acme-insights",
    kind: "microsoft.insights/components",
    label: "Application Insights",
    group: "shared",
    sku: null,
    state: "Succeeded",
  },
];

function inventory(params: Params) {
  const group = params.group as string | undefined;
  const find = String(params.find ?? "").toLowerCase();
  const all = [
    ...MACHINES.map((machine) => ({
      id: machine.id,
      name: machine.name,
      kind: "microsoft.compute/virtualmachines",
      label: "Virtual machine",
      group: machine.group,
      location: machine.location,
      state: machine.power,
      sku: machine.size,
      tags: machine.tags,
    })),
    ...APPS.map((app) => ({
      id: app.id,
      name: app.name,
      kind: "microsoft.web/sites",
      label: app.functions ? "Function app" : "Web app",
      group: app.group,
      location: app.location,
      state: app.state,
      sku: null,
      tags: app.tags,
    })),
    ...STORAGE.map((account) => ({
      id: account.id,
      name: account.name,
      kind: "microsoft.storage/storageaccounts",
      label: "Storage account",
      group: account.group,
      location: account.location,
      state: "Succeeded",
      sku: account.sku,
      tags: [] as [string, string][],
    })),
    ...VAULTS.map((vault) => ({
      id: vault.id,
      name: vault.name,
      kind: "microsoft.keyvault/vaults",
      label: "Key vault",
      group: vault.group,
      location: vault.location,
      state: "Succeeded",
      sku: null,
      tags: [] as [string, string][],
    })),
    ...EXTRA.map((extra) => ({
      id: id(extra.group, extra.kind, extra.name),
      name: extra.name,
      kind: extra.kind,
      label: extra.label,
      group: extra.group,
      location: extra.group === "shared" ? "centralindia" : "uaenorth",
      state: extra.state,
      sku: extra.sku,
      tags: [] as [string, string][],
    })),
  ];
  const resources = all.filter(
    (resource) =>
      (!group || resource.group === group) &&
      (!find || resource.name.toLowerCase().includes(find)),
  );
  const kinds: { kind: string; label: string; count: number }[] = [];
  for (const resource of resources) {
    const found = kinds.find((each) => each.kind === resource.kind);
    if (found) found.count += 1;
    else kinds.push({ kind: resource.kind, label: resource.label, count: 1 });
  }
  kinds.sort((a, b) => b.count - a.count || a.label.localeCompare(b.label));
  return { resources, kinds, truncated: false };
}

const inGroup = <T extends { group: string }>(items: T[], params: Params) => {
  const group = params.group as string | undefined;
  return group ? items.filter((item) => item.group === group) : items;
};

const LOG_LINES = [
  "2026-10-19T19:04:01 [Information] Application started. Press Ctrl+C to shut down.",
  "2026-10-19T19:04:01 [Information] Hosting environment: Production",
  "2026-10-19T19:04:12 [Information] GET / responded 200 in 38ms",
  "2026-10-19T19:04:14 [Information] GET /products/sneakers responded 200 in 64ms",
  "2026-10-19T19:04:19 [Warning] Upstream /api/cart answered 404, falling back to the empty cart",
  "2026-10-19T19:04:22 [Information] POST /checkout responded 200 in 210ms",
];

export const AZURE: Record<string, (params: Params) => unknown> = {
  status: () => ({
    ready: true,
    configured: true,
    authFailed: false,
    category: null,
    message: null,
    user: "edon@acme.dev",
    cliVersion: "2.90.0",
    subscriptions: [
      {
        id: SUBSCRIPTION,
        name: "Acme production",
        tenantId: "t1",
        state: "Enabled",
        isDefault: true,
        user: "edon@acme.dev",
      },
      {
        id: "2d7e9f01-aaaa-bbbb-cccc-1234567890ab",
        name: "Acme sandbox",
        tenantId: "t1",
        state: "Enabled",
        isDefault: false,
        user: "edon@acme.dev",
      },
    ],
    current: {
      id: SUBSCRIPTION,
      name: "Acme production",
      tenantId: "t1",
      state: "Enabled",
      isDefault: true,
      user: "edon@acme.dev",
    },
  }),
  groups: () => GROUPS,
  resources: inventory,
  vms: (params) => inGroup(MACHINES, params),
  vm: (params) =>
    MACHINES.find((machine) => machine.name === params.name) ?? MACHINES[0],
  vmAct: (params) => ({
    message: `${String(params.name)} is ${params.action === "start" ? "starting" : "stopping"}`,
  }),
  apps: (params) => inGroup(APPS, params),
  app: (params) => {
    const app = APPS.find((each) => each.name === params.name) ?? APPS[0];
    return {
      ...app,
      alwaysOn: true,
      inboundIp: "20.74.12.40",
      outboundIps: ["20.74.12.41", "20.74.12.42", "20.74.12.43"],
      slots: app.functions
        ? []
        : [
            {
              name: "staging",
              state: "Running",
              host: `${app.name}-staging.azurewebsites.net`,
            },
          ],
      settings: [
        { name: "APPLICATIONINSIGHTS_CONNECTION_STRING", fromKeyVault: false },
        { name: "DATABASE_URL", fromKeyVault: true },
        { name: "NODE_ENV", fromKeyVault: false },
        { name: "STRIPE_SECRET_KEY", fromKeyVault: true },
      ],
    };
  },
  appAct: (params) => ({ message: `${String(params.name)} is restarting` }),
  storage: (params) => inGroup(STORAGE, params),
  containers: (params) => CONTAINERS[String(params.name)] ?? [],
  vaults: (params) => inGroup(VAULTS, params),
  activity: (params) => {
    const events = inGroup(ACTIVITY, params);
    return params.failedOnly
      ? events.filter(
          (event) => event.status === "Failed" || event.level === "Error",
        )
      : events;
  },
  metrics: () => [],
};

export function azureLogLines(): { line: string }[] {
  return LOG_LINES.map((line) => ({ line }));
}
