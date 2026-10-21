type Params = Record<string, unknown>;

const minutesAgo = (minutes: number) => Date.now() - minutes * 60_000;

const TEAM = { id: "team_acme", slug: "acme", name: "Acme Labs" };

const PROJECTS = [
  {
    id: "prj_storefront",
    name: "storefront",
    framework: "nextjs",
    updatedAt: minutesAgo(3),
    repo: "acme/storefront",
    gitProvider: "github",
    productionBranch: "main",
    production: {
      id: "dpl_live",
      url: "https://shop.acme.dev",
      state: "READY",
      createdAt: minutesAgo(95),
    },
    latestProduction: null,
  },
  {
    id: "prj_docs",
    name: "docs",
    framework: "astro",
    updatedAt: minutesAgo(40),
    repo: "acme/docs",
    gitProvider: "github",
    productionBranch: "main",
    production: {
      id: "dpl_docs",
      url: "https://docs.acme.dev",
      state: "READY",
      createdAt: minutesAgo(40),
    },
    latestProduction: null,
  },
  {
    id: "prj_dashboard",
    name: "dashboard",
    framework: "vite",
    updatedAt: minutesAgo(60 * 26),
    repo: "acme/dashboard",
    gitProvider: "github",
    productionBranch: "main",
    production: {
      id: "dpl_dash",
      url: "https://app.acme.dev",
      state: "READY",
      createdAt: minutesAgo(60 * 50),
    },
    latestProduction: {
      id: "dpl_dash_fail",
      state: "ERROR",
      createdAt: minutesAgo(60 * 26),
    },
  },
];

const deployment = (
  id: string,
  state: string,
  target: "production" | "preview",
  minutes: number,
  branch: string,
  commit: string,
  message: string,
  author: string,
  error: string | null = null,
) => ({
  id,
  project: "storefront",
  projectId: "prj_storefront",
  url: `https://storefront-${id.slice(4)}-acme.vercel.app`,
  state,
  target,
  createdAt: minutesAgo(minutes),
  readyAt: state === "READY" ? minutesAgo(minutes - 1) : null,
  buildMs: state === "READY" || state === "ERROR" ? 38_000 + minutes * 7 : null,
  branch,
  commit,
  message,
  author,
  redeployOf: null as string | null,
  inspectorUrl: `https://vercel.com/acme/storefront/${id.slice(4)}`,
  aliases: null,
  error,
});

const DEPLOYMENTS = [
  deployment(
    "dpl_build",
    "BUILDING",
    "preview",
    1,
    "cart-drawer",
    "9f1c2ab0e4",
    "Slide the cart in from the right",
    "maya",
  ),
  deployment(
    "dpl_fail",
    "ERROR",
    "preview",
    18,
    "checkout-totals",
    "4be71d03aa",
    "Show tax beside the total",
    "edon",
    'Command "pnpm build" exited with 1',
  ),
  deployment(
    "dpl_live",
    "READY",
    "production",
    95,
    "main",
    "c03e9d41f7",
    "Merge pull request #212 from acme/search-filters",
    "lena",
  ),
  deployment(
    "dpl_prev",
    "READY",
    "production",
    60 * 22,
    "main",
    "71aa0b5c2e",
    "Ship the new product grid",
    "sam",
  ),
  deployment(
    "dpl_stop",
    "CANCELED",
    "preview",
    60 * 23,
    "grid-spacing",
    "e5d28f9b41",
    "Try a wider gutter",
    "sam",
  ),
  deployment(
    "dpl_old",
    "READY",
    "preview",
    60 * 30,
    "search-filters",
    "b28c11e6d0",
    "Filter by size and colour",
    "lena",
  ),
];

const FAILED_LOG = [
  ["command", 'Running "pnpm build"'],
  ["stdout", "> storefront@2.4.0 build"],
  ["stdout", "> next build"],
  ["stdout", "   ▲ Next.js 16.1.0"],
  ["stdout", "   Creating an optimized production build ..."],
  ["stdout", " ✓ Compiled successfully in 21.4s"],
  ["stdout", "   Linting and checking validity of types ..."],
  ["stderr", "Failed to compile."],
  ["stderr", ""],
  ["stderr", "./app/checkout/Totals.tsx:42:18"],
  ["stderr", "Type error: Property 'taxRate' does not exist on type 'Cart'."],
  ["stderr", ""],
  ["stderr", "  40 |   const subtotal = sum(cart.lines);"],
  ["stderr", "  41 |   return ("],
  ["stderr", "> 42 |     <Money value={subtotal * cart.taxRate} />"],
  ["stderr", "     |                                   ^"],
  ["stderr", "Next.js build worker exited with code: 1 and signal: null"],
  ["stderr", 'Error: Command "pnpm build" exited with 1'],
];

const READY_LOG = [
  ["command", 'Running "pnpm build"'],
  ["stdout", "   ▲ Next.js 16.1.0"],
  ["stdout", " ✓ Compiled successfully in 19.8s"],
  ["stdout", " ✓ Generating static pages (48/48)"],
  ["stdout", "Route (app)                         Size     First Load JS"],
  ["stdout", "┌ ○ /                               5.2 kB         112 kB"],
  ["stdout", "├ ○ /cart                           3.1 kB         109 kB"],
  ["stdout", "└ ƒ /products/[slug]                6.8 kB         118 kB"],
  ["stderr", "Build Completed in /vercel/output [41s]"],
  ["stdout", "Deploying outputs..."],
  ["stdout", "Deployment completed"],
];

const find = (id: unknown) =>
  DEPLOYMENTS.find((each) => each.id === id) ?? DEPLOYMENTS[2];

export const VERCEL: Record<string, (params: Params) => unknown> = {
  runtimeLogs: () => vercelRuntimeLogs(),
  protection: () => ({
    vercelAuthentication: "Preview deployments",
    password: null,
    canShare: true,
  }),
  shareLink: () => ({
    link: "https://storefront-git-checkout-acme.vercel.app/?x-vercel-protection-bypass=showcase&x-vercel-set-bypass-cookie=samesitenone",
  }),
  firewall: () => ({
    enabled: true,
    rules: [
      {
        id: "rule_wp",
        name: "Block WordPress probes",
        description: "Paths under /wp-admin and /wp-login.php",
        active: true,
        action: "Deny",
      },
      {
        id: "rule_api",
        name: "Rate limit checkout",
        description: "60 requests a minute per IP on /api/checkout",
        active: true,
        action: "Rate limit",
      },
      {
        id: "rule_geo",
        name: "Challenge new regions",
        description: null,
        active: false,
        action: "Challenge",
      },
    ],
    blocks: [
      { ip: "203.0.113.7", hostname: null, note: "Card-testing bot", action: "deny" },
    ],
    attackMode: false,
    attackModeUntil: null,
  }),
  setAttackMode: ({ enabled }) => ({
    ...(VERCEL.firewall!({}) as object),
    attackMode: enabled,
  }),
  crons: () => ({
    enabled: true,
    deploymentId: "dpl_live",
    updatedAt: Date.now() - 95 * 60_000,
    jobs: [
      { path: "/api/cron/sync-inventory", schedule: "*/15 * * * *", host: null },
      { path: "/api/cron/abandoned-carts", schedule: "0 9 * * *", host: null },
      { path: "/api/cron/weekly-report", schedule: "0 6 * * 1", host: null },
    ],
  }),
  domain: ({ domain }) =>
    String(domain).startsWith("staging.")
      ? {
          name: domain,
          verified: false,
          misconfigured: true,
          redirect: null,
          redirectStatusCode: null,
          gitBranch: "staging",
          records: [
            {
              type: "TXT",
              name: "_vercel.shop.acme.dev",
              value: "vc-domain-verify=staging.shop.acme.dev,8f2c41d9e0",
              why: "Proves the domain is yours",
            },
            {
              type: "CNAME",
              name: "staging.shop",
              value: "cname.vercel-dns.com",
              why: "Points the domain at Vercel",
            },
          ],
        }
      : {
          name: domain,
          verified: true,
          misconfigured: false,
          redirect: null,
          redirectStatusCode: null,
          gitBranch: null,
          records: [],
        },
  addDomain: ({ domain }) => VERCEL.domain!({ domain }),
  updateDomain: ({ domain }) => VERCEL.domain!({ domain }),
  verifyDomain: ({ domain }) => VERCEL.domain!({ domain }),
  removeDomain: () => null,
  setEnv: ({ key, targets, branch }) => ({
    id: `env_${String(key)}`,
    key,
    target: targets,
    type: "encrypted",
    gitBranch: branch ?? null,
    updatedAt: Date.now(),
  }),
  removeEnv: () => null,
  settings: ({ project }) => ({
    id: `prj_${String(project)}`,
    name: project,
    framework: "nextjs",
    buildCommand: null,
    outputDirectory: null,
    installCommand: "pnpm install --frozen-lockfile",
    devCommand: null,
    rootDirectory: "apps/storefront",
    nodeVersion: "22.x",
    nodeVersions: ["24.x", "22.x", "20.x"],
  }),
  updateSettings: ({ project, settings }) => ({
    ...(VERCEL.settings!({ project }) as object),
    ...(settings as object),
  }),
  status: () => ({
    configured: true,
    ok: true,
    authFailed: false,
    message: null,
    account: {
      id: "u_edon",
      username: "edon",
      name: "Edon",
      email: "edon@acme.dev",
    },
    teams: [TEAM],
    team: TEAM.id,
    personal: false,
  }),
  setTeam: () => null,
  projects: () => PROJECTS,
  deployments: () => DEPLOYMENTS,
  deployment: ({ id }) => find(id),
  logs: ({ id }) => {
    const found = find(id);
    const lines =
      found.state === "ERROR"
        ? FAILED_LOG
        : found.state === "BUILDING"
          ? FAILED_LOG.slice(0, 5)
          : READY_LOG;
    return {
      deployment: found,
      lines: lines.map(([kind, text], index) => ({
        at: found.createdAt + index * 900,
        kind,
        text,
      })),
      skipped: 0,
    };
  },
  domains: () => [
    { name: "shop.acme.dev", verified: true, redirect: null, gitBranch: null },
    {
      name: "www.shop.acme.dev",
      verified: true,
      redirect: "shop.acme.dev",
      gitBranch: null,
    },
    {
      name: "staging.shop.acme.dev",
      verified: false,
      redirect: null,
      gitBranch: "staging",
    },
  ],
  env: () => [
    {
      id: "e1",
      key: "DATABASE_URL",
      target: ["production", "preview"],
      type: "encrypted",
      gitBranch: null,
      updatedAt: minutesAgo(9000),
    },
    {
      id: "e2",
      key: "STRIPE_SECRET_KEY",
      target: ["production"],
      type: "sensitive",
      gitBranch: null,
      updatedAt: minutesAgo(9000),
    },
    {
      id: "e3",
      key: "NEXT_PUBLIC_SITE_URL",
      target: ["production", "preview", "development"],
      type: "plain",
      gitBranch: null,
      updatedAt: minutesAgo(400),
    },
    {
      id: "e4",
      key: "SEARCH_INDEX",
      target: ["preview"],
      type: "encrypted",
      gitBranch: "search-filters",
      updatedAt: minutesAgo(1800),
    },
  ],
  redeploy: ({ id }) => find(id),
  promote: ({ id }) => find(id),
  cancel: ({ id }) => find(id),
};

export function vercelRuntimeLogs() {
  const now = Date.now();
  const line = (
    ago: number,
    kind: "request" | "log",
    level: string,
    method: string,
    path: string,
    status: number | null,
    message: string,
    requestId: string,
  ) => ({
    at: new Date(now - ago).toISOString(),
    kind,
    level,
    message,
    method,
    path,
    status,
    requestId,
    deploymentId: "dpl_live",
  });
  return {
    lines: [
      line(1_200, "log", "error", "POST", "/api/checkout", null, "TypeError: Cannot read properties of undefined (reading 'items')\n    at checkout (/var/task/app/api/checkout/route.js:18:27)", "r5"),
      line(1_300, "request", "error", "POST", "/api/checkout", 500, "/api/checkout · streaming_func · MISS", "r5"),
      line(3_000, "request", "info", "POST", "/api/cart", 200, "/api/cart · streaming_func · MISS", "r4"),
      line(5_100, "request", "warning", "GET", "/favicon.png", 404, "/[slug] · prerender · MISS", "r3"),
      line(7_400, "request", "info", "GET", "/api/products", 200, "/api/products · streaming_func · HIT", "r2"),
      line(9_000, "request", "info", "GET", "/", 200, "/ · prerender · HIT", "r1"),
    ],
    minutes: 15,
    filtered: 0,
    note: null,
  };
}

export function vercelDeploySteps() {
  return [
    { step: "Reading files", done: 0, total: 0, deployment: null },
    { step: "Uploading", done: 0, total: 3, deployment: null },
    { step: "Uploading", done: 3, total: 3, deployment: null },
    {
      step: "Building",
      done: 214,
      total: 214,
      deployment: DEPLOYMENTS[0],
    },
  ];
}
