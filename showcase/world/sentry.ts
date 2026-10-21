type Params = Record<string, unknown>;

const ago = (minutes: number) =>
  new Date(Date.now() - minutes * 60_000).toISOString();

const PROJECTS = [
  {
    id: "11",
    slug: "storefront",
    name: "Storefront",
    platform: "javascript-nextjs",
  },
  {
    id: "12",
    slug: "checkout-api",
    name: "Checkout API",
    platform: "python-django",
  },
  { id: "13", slug: "order-worker", name: "Order worker", platform: "go" },
];

const MEMBERS = [
  { actor: "user:42", name: "Edon Ramos", email: "edon@acme.dev", isMe: true },
  { actor: "user:7", name: "Maya Chen", email: "maya@acme.dev", isMe: false },
  {
    actor: "user:9",
    name: "Tomás Silva",
    email: "tomas@acme.dev",
    isMe: false,
  },
];

const trend = (shape: number[]) =>
  Array.from({ length: 24 }, (_, hour) => shape[hour % shape.length] ?? 0);

const issue = (id: string, patch: Params) => ({
  id,
  shortId: "",
  title: "",
  culprit: "",
  kind: null,
  message: null,
  level: "error",
  status: "unresolved",
  substatus: "ongoing",
  events: 0,
  users: 0,
  firstSeen: ago(60 * 24 * 6),
  lastSeen: ago(3),
  project: "storefront",
  link: `https://acme.sentry.io/issues/${id}/`,
  assignee: null as { kind: string; id: string; name: string } | null,
  unhandled: false,
  trend: trend([1, 2, 1, 3]),
  ...patch,
});

const ISSUES = [
  issue("4512887211", {
    shortId: "STOREFRONT-3F",
    title: "TypeError: Cannot read properties of undefined (reading 'total')",
    culprit: "app/checkout/summary.ts in totalOf",
    kind: "TypeError",
    message: "Cannot read properties of undefined (reading 'total')",
    substatus: "regressed",
    events: 1284,
    users: 312,
    firstSeen: ago(60 * 24 * 7),
    lastSeen: ago(1),
    unhandled: true,
    assignee: { kind: "user", id: "42", name: "Edon Ramos" },
    trend: [
      2, 1, 0, 3, 2, 4, 3, 2, 1, 2, 3, 2, 5, 9, 14, 22, 41, 66, 80, 92, 101, 87,
      95, 110,
    ],
  }),
  issue("4512901877", {
    shortId: "CHECKOUT-API-1A",
    title:
      'OperationalError: could not obtain lock on row in relation "orders"',
    culprit: "orders.views in capture",
    kind: "OperationalError",
    message: 'could not obtain lock on row in relation "orders"',
    substatus: "escalating",
    project: "checkout-api",
    events: 402,
    users: 118,
    firstSeen: ago(60 * 26),
    lastSeen: ago(6),
    unhandled: true,
    trend: [
      0, 0, 0, 0, 1, 0, 2, 4, 3, 8, 12, 19, 22, 30, 28, 33, 40, 36, 31, 29, 27,
      24, 22, 20,
    ],
  }),
  issue("4512903390", {
    shortId: "ORDER-WORKER-9",
    title: "panic: capture failed for order: unexpected status 404",
    culprit: "main.(*Worker).capture",
    kind: "panic",
    message: "capture failed for order: unexpected status 404",
    level: "fatal",
    substatus: "new",
    project: "order-worker",
    events: 212,
    users: 0,
    firstSeen: ago(48),
    lastSeen: ago(2),
    unhandled: true,
    assignee: { kind: "user", id: "7", name: "Maya Chen" },
    trend: [
      0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 40, 88, 84,
    ],
  }),
  issue("4512800541", {
    shortId: "STOREFRONT-2Z",
    title: "ChunkLoadError: Loading chunk 8817 failed.",
    culprit: "/_next/static/chunks/8817-4b1f.js",
    kind: "ChunkLoadError",
    message: "Loading chunk 8817 failed.",
    level: "warning",
    events: 96,
    users: 71,
    firstSeen: ago(60 * 24 * 12),
    lastSeen: ago(40),
    trend: [
      4, 3, 5, 2, 6, 4, 3, 5, 4, 2, 3, 6, 5, 4, 3, 4, 5, 3, 4, 6, 2, 3, 4, 5,
    ],
  }),
  issue("4512799012", {
    shortId: "STOREFRONT-2W",
    title:
      "Hydration failed because the server rendered HTML didn't match the client.",
    culprit: "app/(shop)/product/[slug]/page.tsx",
    level: "warning",
    events: 58,
    users: 40,
    firstSeen: ago(60 * 24 * 20),
    lastSeen: ago(60 * 3),
    trend: [
      1, 0, 2, 3, 1, 2, 0, 1, 3, 2, 1, 2, 4, 2, 3, 1, 2, 3, 1, 0, 2, 3, 2, 1,
    ],
  }),
  issue("4512788120", {
    shortId: "CHECKOUT-API-14",
    title: "Slow DB query: SELECT … FROM orders WHERE customer_id = %s",
    culprit: "orders.history",
    level: "info",
    project: "checkout-api",
    events: 31,
    users: 22,
    firstSeen: ago(60 * 24 * 30),
    lastSeen: ago(60 * 9),
    trend: [
      1, 1, 2, 1, 0, 1, 2, 1, 1, 0, 2, 1, 1, 2, 1, 0, 1, 1, 2, 1, 1, 0, 1, 2,
    ],
  }),
];

const STOREFRONT_EVENT = {
  id: "9fd3c1e04a1b4c2f",
  at: ago(1),
  message: null,
  release: "storefront@4.13.0",
  environment: "production",
  request: "GET https://shop.acme.dev/checkout",
  contexts: [
    ["Browser", "Chrome 128.0"],
    ["OS", "macOS 15.1"],
  ],
  tags: [
    ["environment", "production"],
    ["release", "storefront@4.13.0"],
    ["browser", "Chrome 128"],
    ["os", "macOS 15.1"],
    ["url", "https://shop.acme.dev/checkout"],
    ["transaction", "/checkout"],
    ["handled", "no"],
    ["level", "error"],
  ],
  breadcrumbs: [
    {
      at: ago(1.4),
      category: "navigation",
      level: "info",
      message: "from: /cart, to: /checkout",
    },
    {
      at: ago(1.3),
      category: "ui.click",
      level: "info",
      message: "button.checkout-continue",
    },
    {
      at: ago(1.2),
      category: "fetch",
      level: "info",
      message: "GET /api/cart [200]",
    },
    {
      at: ago(1.15),
      category: "fetch",
      level: "warning",
      message: "GET /api/orders/draft [404]",
    },
    {
      at: ago(1.1),
      category: "console",
      level: "error",
      message: "draft order missing, falling back to cart",
    },
  ],
  exceptions: [
    {
      kind: "TypeError",
      value: "Cannot read properties of undefined (reading 'total')",
      handled: false,
      frames: [
        {
          file: "node_modules/react-dom/cjs/react-dom-client.production.js",
          module: null,
          function: "commitRootImpl",
          line: 12407,
          column: 9,
          inApp: false,
          context: [],
        },
        {
          file: "node_modules/react-dom/cjs/react-dom-client.production.js",
          module: null,
          function: "renderWithHooks",
          line: 5014,
          column: 22,
          inApp: false,
          context: [],
        },
        {
          file: "app/checkout/Summary.tsx",
          module: null,
          function: "Summary",
          line: 31,
          column: 18,
          inApp: true,
          context: [
            [28, "export function Summary() {"],
            [29, "    const cart = useCart();"],
            [30, "    const draft = useDraftOrder(cart.id);"],
            [31, "    const total = totalOf(draft.order);"],
            [
              32,
              "    return <Total value={total} currency={cart.currency} />;",
            ],
            [33, "}"],
          ],
        },
        {
          file: "app/checkout/summary.ts",
          module: null,
          function: "totalOf",
          line: 12,
          column: 22,
          inApp: true,
          context: [
            [9, "export function totalOf(order: Order | undefined): Money {"],
            [10, "    const lines = order?.lines;"],
            [
              11,
              "    // lines.total is filled by the API once the draft exists",
            ],
            [12, "    return lines.total;"],
            [13, "}"],
          ],
        },
      ],
    },
  ],
};

function detailOf(id: string) {
  const found = ISSUES.find((each) => each.id === id) ?? ISSUES[0];
  return {
    ...found,
    firstRelease: found.project === "storefront" ? "storefront@4.12.0" : null,
    lastRelease: found.project === "storefront" ? "storefront@4.13.0" : null,
    comments: found.id === "4512887211" ? 2 : 0,
    event:
      found.id === "4512887211"
        ? STOREFRONT_EVENT
        : {
            ...STOREFRONT_EVENT,
            exceptions: [],
            message: found.title,
            breadcrumbs: [],
          },
  };
}

const RELEASES = [
  {
    version: "storefront@4.13.0+a1b2c3d",
    short: "4.13.0 (a1b2c3d)",
    created: ago(60 * 5),
    newIssues: 2,
    commits: 14,
    deployedTo: "production",
    deployedAt: ago(60 * 4.6),
    crashFreeSessions: 97.42,
    crashFreeUsers: 98.1,
    sessions: 18233,
  },
  {
    version: "storefront@4.12.2+9e8d7c6",
    short: "4.12.2 (9e8d7c6)",
    created: ago(60 * 30),
    newIssues: 0,
    commits: 3,
    deployedTo: "production",
    deployedAt: ago(60 * 29),
    crashFreeSessions: 99.81,
    crashFreeUsers: 99.9,
    sessions: 40512,
  },
  {
    version: "storefront@4.12.1+5f4e3d2",
    short: "4.12.1 (5f4e3d2)",
    created: ago(60 * 24 * 3),
    newIssues: 1,
    commits: 8,
    deployedTo: "production",
    deployedAt: ago(60 * 24 * 3 - 20),
    crashFreeSessions: 99.62,
    crashFreeUsers: 99.7,
    sessions: 61200,
  },
  {
    version: "storefront@4.12.0+1a2b3c4",
    short: "4.12.0 (1a2b3c4)",
    created: ago(60 * 24 * 7),
    newIssues: 4,
    commits: 31,
    deployedTo: "production",
    deployedAt: ago(60 * 24 * 7 - 30),
    crashFreeSessions: 98.9,
    crashFreeUsers: 99.2,
    sessions: 88410,
  },
  {
    version: "storefront@4.13.0-rc.1",
    short: "4.13.0-rc.1",
    created: ago(60 * 9),
    newIssues: 0,
    commits: 12,
    deployedTo: "staging",
    deployedAt: ago(60 * 8.7),
    crashFreeSessions: 100,
    crashFreeUsers: 100,
    sessions: 212,
  },
];

function issuesFor(params: Params) {
  const ids = (params.projects as string[] | undefined) ?? [];
  const slugs = PROJECTS.filter((project) => ids.includes(project.id)).map(
    (project) => project.slug,
  );
  const status = (params.status as string | undefined) ?? "unresolved";
  const words = String(params.query ?? "").toLowerCase();
  const found = ISSUES.filter(
    (each) =>
      (slugs.length === 0 || slugs.includes(each.project)) &&
      (status === "all" ||
        status === "unresolved" ||
        (status === "assigned" && each.assignee?.id === "42")) &&
      (!words ||
        `${each.title} ${each.culprit} ${each.shortId}`
          .toLowerCase()
          .includes(words)),
  );
  const sort = params.sort as string | undefined;
  const key = (each: (typeof ISSUES)[number]) =>
    sort === "freq"
      ? each.events
      : sort === "user"
        ? each.users
        : sort === "new"
          ? Date.parse(each.firstSeen)
          : Date.parse(each.lastSeen);
  return { issues: [...found].sort((a, b) => key(b) - key(a)), next: null };
}

export const SENTRY: Record<string, (params: Params) => unknown> = {
  status: () => ({
    configured: true,
    ok: true,
    authFailed: false,
    message: null,
    url: "https://sentry.io",
    org: "acme",
    orgName: "Acme",
    project: "storefront",
    user: "Edon Ramos",
  }),
  organizations: () => [{ slug: "acme", name: "Acme" }],
  choose: () => SENTRY.status({}),
  projects: () => PROJECTS,
  members: () => MEMBERS,
  environments: () => ["production", "staging", "preview"],
  issues: issuesFor,
  issue: (params) => detailOf(String(params.id)),
  brief: (params) => {
    const detail = detailOf(String(params.id));
    return `${detail.shortId} · ${detail.title}\n${detail.link}`;
  },
  change: (params) => ({
    ...detailOf(String(params.id)),
    status: params.action === "unresolve" ? "unresolved" : "resolved",
  }),
  comment: () => null,
  releases: () => RELEASES,
};
