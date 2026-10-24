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
].map((name) => ({
  name,
  kind: "table",
  kindName: "table",
  preview: `select * from "public"."${name}" limit 100;`,
  columns: null,
  failed: null,
}));

const CONNECTION = {
  id: PROFILE.id,
  name: PROFILE.name,
  engine: "postgres",
  engineName: "PostgreSQL",
  address: "readonly@db.internal.acme.dev:5432/shop",
  readOnly: true,
  connected: "PostgreSQL 17.2",
  defaultSchema: "public",
  facts: [
    { label: "Engine", value: "PostgreSQL", mono: false },
    { label: "Server", value: "db.internal.acme.dev:5432", mono: true },
    { label: "Database", value: "shop", mono: true },
    { label: "User", value: "readonly", mono: true },
    { label: "Password", value: "Saved in the Keychain", mono: false },
    { label: "Agents", value: "Read only", mono: false },
    { label: "Status", value: "Connected to PostgreSQL 17.2", mono: false },
  ],
};

type Opened = { id: string; schemas: string[] };

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
  form: () => ({
    draft: {
      id: PROFILE.id,
      name: PROFILE.name,
      engine: "postgres",
      host: PROFILE.host,
      port: String(PROFILE.port),
      database: PROFILE.database,
      user: PROFILE.user,
      tls: PROFILE.tls,
      path: "",
      readOnly: PROFILE.readOnly,
      agentWrites: PROFILE.agentWrites,
    },
    hasPassword: PROFILE.hasPassword,
    engines: [
      {
        engine: "postgres",
        label: "PostgreSQL",
        defaultPort: 5432,
        nameHint: "Production",
        userHint: "postgres",
        databaseHint: "Same as the user name",
        pathHint: "",
      },
    ],
    tlsModes: [
      {
        value: "require",
        label: "Require",
        detail: "Always encrypt, any certificate",
      },
    ],
  }),
  connect: () => null,
  explorerView: ({ open }) => {
    const opened = (open as Opened[]).find((each) => each.id === PROFILE.id);
    const schemas = ["public", "analytics"].map((name) => ({
      name,
      groups: opened?.schemas.includes(name)
        ? name === "public"
          ? [{ name: "tables", tables: TABLES }]
          : []
        : null,
      failed: null,
    }));
    return {
      connections: [
        { ...CONNECTION, schemas: opened ? schemas : null, failed: null },
      ],
    };
  },
  tableView: () => ({
    schema: "public",
    name: "orders",
    kind: "table",
    kindName: "table",
    preview: 'select * from "public"."orders" limit 100;',
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
        rows: ROWS.map((row) => row.map(String)),
        truncated: false,
        affected: null,
        summary: `${ROWS.length} rows`,
        forAgent: `Results of a query on the PostgreSQL database "${PROFILE.name}"`,
      },
    ],
    main: 0,
  }),
  copyResult: () => "",
  historyView: () => ({
    entries: [],
    empty: "Queries you and your agents run here show up in this list.",
  }),
};
