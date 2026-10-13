type Args = Record<string, unknown>;

const PROFILE = {
  id: "db-shop",
  name: "shop (production)",
  engine: "postgres",
  host: "db.internal.acme.dev",
  port: 5432,
  database: "shop",
  user: "readonly",
  tls: "require",
  readOnly: true,
  agentWrites: false,
  hasPassword: true,
};

export const DATABASE_QUERY = `select o.id, c.email, o.total_cents / 100.0 as total, o.status, o.created_at
from orders o
join customers c on c.id = o.customer_id
where o.status = 'failed'
  and o.created_at > now() - interval '24 hours'
order by o.created_at desc;`;

const TABLES = [
  "customers",
  "invoices",
  "line_items",
  "orders",
  "payments",
  "products",
  "refunds",
  "webhooks",
].map((name) => ({ name, kind: "table" }));

const ROWS: [number, string, string, string, string][] = [
  [48213, "maya@northwind.io", "129.00", "failed", "2026-09-26 09:38:12"],
  [48207, "li.wei@fabrikam.com", "42.50", "failed", "2026-09-26 09:31:47"],
  [48199, "sam@contoso.dev", "310.00", "failed", "2026-09-26 09:22:05"],
  [48188, "priya@tailspin.app", "18.99", "failed", "2026-09-26 09:12:40"],
  [48176, "jonas@lumen.studio", "76.20", "failed", "2026-09-26 08:58:19"],
  [48160, "ana@adatum.co", "245.00", "failed", "2026-09-26 08:41:02"],
  [48151, "omar@woodgrove.net", "12.00", "failed", "2026-09-26 08:33:57"],
  [48139, "kate@litware.org", "89.90", "failed", "2026-09-26 08:20:11"],
  [48124, "diego@proseware.io", "158.40", "failed", "2026-09-26 08:04:33"],
  [48110, "eli@fourthcoffee.com", "33.75", "failed", "2026-09-26 07:49:26"],
  [48097, "noor@wingtip.dev", "510.00", "failed", "2026-09-26 07:35:08"],
];

export const DATABASE: Record<string, (params: Args) => unknown> = {
  profiles: () => [PROFILE],
  connected: () => [{ id: PROFILE.id, version: "PostgreSQL 17.2" }],
  connect: () => ({ id: PROFILE.id, version: "PostgreSQL 17.2" }),
  schemas: () => ["public", "analytics"],
  tables: ({ schema }) => (schema === "public" ? TABLES : []),
  describe: () => ({
    schema: "public",
    name: "orders",
    kind: "table",
    columns: [],
    indexes: [],
    foreignKeys: [],
  }),
  query: () => ({
    millis: 38,
    results: [
      {
        columns: [
          { name: "id", type: "int8", numeric: true },
          { name: "email", type: "text", numeric: false },
          { name: "total", type: "numeric", numeric: true },
          { name: "status", type: "order_status", numeric: false },
          { name: "created_at", type: "timestamptz", numeric: false },
        ],
        rows: ROWS,
        truncated: false,
        affected: null,
      },
    ],
  }),
  history: () => [],
};
