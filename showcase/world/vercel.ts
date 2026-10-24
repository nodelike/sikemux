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
  projectsView: () => ({ projects: PROJECTS }),
  deploymentsView: () => ({ deployments: DEPLOYMENTS }),
  logView: ({ id }) => {
    const found = find(id);
    const lines =
      found.state === "ERROR"
        ? FAILED_LOG
        : found.state === "BUILDING"
          ? FAILED_LOG.slice(0, 5)
          : READY_LOG;
    return {
      log: {
        deployment: found,
        lines: lines.map(([kind, text], index) => ({
          at: found.createdAt + index * 900,
          kind,
          text,
        })),
        skipped: 0,
      },
      forAgent: `Vercel deployment ${found.id} of ${found.project}`,
    };
  },
  domainsView: () => ({
    domains: [
      {
        name: "shop.acme.dev",
        verified: true,
        redirect: null,
        gitBranch: null,
      },
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
  }),
  envView: () => ({
    variables: [
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
  }),
  redeploy: ({ id }) => find(id),
  promote: ({ id }) => find(id),
  cancel: ({ id }) => find(id),
};
