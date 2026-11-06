import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import pg from "pg";
import { expect, it } from "vitest";

import { openDatabase } from "../src/db.ts";
import { migrate, readMigrations } from "../src/migrations.ts";
import { freshDatabase, log } from "./support.ts";

const MIGRATIONS = fileURLToPath(new URL("../migrations", import.meta.url));
const SCHEMA = fileURLToPath(new URL("../schema.dbml", import.meta.url));

interface Column {
  table: string;
  name: string;
  type: string;
  notNull: boolean;
  defaultExpr: string | null;
  identity: boolean;
}

interface Constraint {
  table: string;
  type: "p" | "u" | "f" | "c";
  columns: string[];
  definition: string;
  refTable: string | null;
  refColumns: string[] | null;
  onDelete: string;
}

interface Index {
  table: string;
  name: string;
  columns: string[];
  descending: boolean[];
  unique: boolean;
  predicate: string | null;
}

const TYPES: Record<string, string> = {
  "timestamp with time zone": "timestamptz",
  "timestamp without time zone": "timestamp",
};

const ON_DELETE: Record<string, string> = {
  c: "cascade",
  n: "set null",
  d: "set default",
  r: "restrict",
};

function quote(text: string): string {
  return `'${text.replaceAll("\\", "\\\\").replaceAll("'", "\\'")}'`;
}

function tidy(sql: string): string {
  return sql
    .replace(/::(text|bigint|integer|jsonb)(\[\])?/g, "")
    .replace(/^CHECK \(\((.*)\)\)$/, "$1")
    .replace(/^CHECK \((.*)\)$/, "$1");
}

function defaultSetting(column: Column): string | null {
  const value = column.defaultExpr;
  if (value === null || value.startsWith("nextval(")) return null;
  if (/^-?\d+(\.\d+)?$/.test(value)) return `default: ${value}`;
  const literal = /^'(.*)'::[a-z ]+$/.exec(value);
  if (literal?.[1] !== undefined) return `default: ${quote(literal[1])}`;
  return `default: \`${value}\``;
}

async function render(url: string): Promise<string> {
  const client = new pg.Client({ connectionString: url });
  await client.connect();
  try {
    const tables = await client.query<{
      name: string;
      comment: string | null;
    }>(`
      select c.relname as name, obj_description(c.oid, 'pg_class') as comment
      from pg_class c join pg_namespace n on n.oid = c.relnamespace
      where n.nspname = 'public' and c.relkind = 'r' and c.relname <> 'schema_migrations'
      order by c.oid`);
    const columns = await client.query<Column>(`
      select c.relname as table, a.attname as name,
        format_type(a.atttypid, a.atttypmod) as type, a.attnotnull as "notNull",
        pg_get_expr(d.adbin, d.adrelid) as "defaultExpr", a.attidentity <> '' as identity
      from pg_attribute a
      join pg_class c on c.oid = a.attrelid
      join pg_namespace n on n.oid = c.relnamespace
      left join pg_attrdef d on d.adrelid = a.attrelid and d.adnum = a.attnum
      where n.nspname = 'public' and c.relkind = 'r' and a.attnum > 0 and not a.attisdropped
      order by c.oid, a.attnum`);
    const constraints = await client.query<Constraint>(`
      select c.relname as table, k.contype as type,
        array(select attname from unnest(k.conkey) with ordinality u(num, pos)
          join pg_attribute on attrelid = k.conrelid and attnum = u.num order by u.pos)::text[] as columns,
        pg_get_constraintdef(k.oid) as definition,
        r.relname as "refTable",
        case when k.confrelid <> 0 then array(select attname from unnest(k.confkey) with ordinality u(num, pos)
          join pg_attribute on attrelid = k.confrelid and attnum = u.num order by u.pos)::text[] end as "refColumns",
        k.confdeltype as "onDelete"
      from pg_constraint k
      join pg_class c on c.oid = k.conrelid
      join pg_namespace n on n.oid = c.relnamespace
      left join pg_class r on r.oid = k.confrelid
      where n.nspname = 'public' and k.contype in ('p', 'u', 'f', 'c')
      order by c.oid, k.contype, k.conname`);
    const indexes = await client.query<Index>(`
      select c.relname as table, i.relname as name,
        array(select attname from unnest(x.indkey) with ordinality u(num, pos)
          join pg_attribute on attrelid = x.indrelid and attnum = u.num order by u.pos)::text[] as columns,
        array(select (opt & 1) = 1 from unnest(x.indoption) opt) as descending,
        x.indisunique as unique, pg_get_expr(x.indpred, x.indrelid) as predicate
      from pg_index x
      join pg_class c on c.oid = x.indrelid
      join pg_class i on i.oid = x.indexrelid
      join pg_namespace n on n.oid = c.relnamespace
      where n.nspname = 'public' and c.relname <> 'schema_migrations' and not x.indisprimary
        and not exists (select 1 from pg_constraint k where k.conindid = x.indexrelid)
      order by c.oid, i.relname`);

    const lines = [
      "// Generated from migrations/ by `pnpm schema:generate`. Open it in dbdiagram.io.",
      "",
    ];
    for (const table of tables.rows) {
      const own = constraints.rows.filter((k) => k.table === table.name);
      const single = (type: Constraint["type"], column: string) =>
        own.filter(
          (k) =>
            k.type === type &&
            k.columns.length === 1 &&
            k.columns[0] === column,
        );
      const primary = own.find((k) => k.type === "p");
      const tableChecks = own.filter(
        (k) => k.type === "c" && k.columns.length !== 1,
      );

      lines.push(`Table ${table.name} {`);
      for (const column of columns.rows.filter((c) => c.table === table.name)) {
        const settings: string[] = [];
        if (primary?.columns.length === 1 && primary.columns[0] === column.name)
          settings.push("pk");
        if (column.identity || column.defaultExpr?.startsWith("nextval("))
          settings.push("increment");
        if (column.notNull && !settings.includes("pk"))
          settings.push("not null");
        if (single("u", column.name).length) settings.push("unique");
        const fallback = defaultSetting(column);
        if (fallback) settings.push(fallback);
        const checks = single("c", column.name).map((k) => tidy(k.definition));
        if (checks.length) settings.push(`note: ${quote(checks.join("; "))}`);
        const type = TYPES[column.type] ?? column.type;
        lines.push(
          `  ${column.name} ${type.includes(" ") ? `"${type}"` : type}${settings.length ? ` [${settings.join(", ")}]` : ""}`,
        );
      }

      const blocks: string[] = [];
      if (primary && primary.columns.length > 1)
        blocks.push(`    (${primary.columns.join(", ")}) [pk]`);
      for (const k of own.filter((k) => k.type === "u" && k.columns.length > 1))
        blocks.push(`    (${k.columns.join(", ")}) [unique]`);
      for (const index of indexes.rows.filter((i) => i.table === table.name)) {
        const settings = [`name: ${quote(index.name)}`];
        if (index.unique) settings.push("unique");
        const notes = [
          ...index.columns
            .filter((_, i) => index.descending[i])
            .map((name) => `${name} desc`),
          ...(index.predicate ? [`where ${tidy(index.predicate)}`] : []),
        ];
        if (notes.length) settings.push(`note: ${quote(notes.join("; "))}`);
        const target =
          index.columns.length === 1
            ? index.columns[0]
            : `(${index.columns.join(", ")})`;
        blocks.push(`    ${target} [${settings.join(", ")}]`);
      }
      if (blocks.length) lines.push("", "  indexes {", ...blocks, "  }");

      const notes = [
        ...(table.comment ? [table.comment] : []),
        ...tableChecks.map((k) => tidy(k.definition)),
      ];
      if (notes.length) lines.push("", `  Note: ${quote(notes.join("; "))}`);
      lines.push("}", "");
    }

    for (const k of constraints.rows.filter((k) => k.type === "f")) {
      const ownKeys = constraints.rows.filter(
        (u) =>
          u.table === k.table &&
          (u.type === "p" || u.type === "u") &&
          u.columns.join() === k.columns.join(),
      );
      const side = (columns: string[]) =>
        columns.length === 1 ? columns[0] : `(${columns.join(", ")})`;
      const onDelete = ON_DELETE[k.onDelete];
      lines.push(
        `Ref: ${k.table}.${side(k.columns)} ${ownKeys.length ? "-" : ">"} ${k.refTable}.${side(k.refColumns ?? [])}${onDelete ? ` [delete: ${onDelete}]` : ""}`,
      );
    }
    return `${lines.join("\n")}\n`;
  } finally {
    await client.end();
  }
}

it("schema.dbml matches the migrations", async () => {
  const fresh = await freshDatabase();
  const database = openDatabase(fresh.url, log);
  try {
    await migrate(database.pool, await readMigrations(MIGRATIONS), log);
    const rendered = await render(fresh.url);
    if (process.env.UPDATE_SCHEMA) writeFileSync(SCHEMA, rendered);
    expect(
      rendered,
      "schema.dbml is stale: run `pnpm schema:generate` in server/",
    ).toBe(readFileSync(SCHEMA, "utf8"));
  } finally {
    await database.close();
    await fresh.drop();
  }
});
