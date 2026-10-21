import {
  Kysely,
  PostgresDialect,
  sql,
  type ColumnType,
  type Generated,
} from "kysely";
import pg from "pg";

import type { Logger } from "./log.ts";

type CreatedAt = ColumnType<Date, never, never>;

/** The tables the API reads and writes. Each one arrives with the migration that creates it. */
export interface Tables {
  users: {
    id: string;
    created_at: CreatedAt;
    deleted_at: ColumnType<Date | null, never, Date | null>;
    clerk_deleted_at: ColumnType<Date | null, never, Date | null>;
    clerk_attempts: ColumnType<number, never, number>;
    clerk_retry_at: ColumnType<Date | null, never, Date | null>;
    purge_after: ColumnType<Date | null, never, Date | null>;
    events_pruned_through: ColumnType<string, never, string | number>;
  };
  events: {
    id: Generated<string>;
    user_id: string;
    type: string;
    subject: string | null;
    subject_role: string | null;
    reason: string | null;
    at: CreatedAt;
  };
  removed_devices: {
    key: string;
    user_id: string;
    role: string;
    reason: string;
    acked_event_id: string | number;
    clerk_session_id: string | null;
    clerk_revoked_at: ColumnType<Date | null, never, Date | null>;
    clerk_attempts: ColumnType<number, never, number>;
    clerk_retry_at: ColumnType<
      Date | null,
      Date | null | undefined,
      Date | null
    >;
    removed_at: CreatedAt;
  };
  devices: {
    key: string;
    user_id: string;
    role: string;
    name: string;
    platform: string;
    channel: string | null;
    created_at: CreatedAt;
    updated_at: ColumnType<Date, never, Date>;
    last_seen_at: ColumnType<Date | null, never, Date | null>;
    acked_event_id: ColumnType<
      string,
      string | number | undefined,
      string | number
    >;
    clerk_session_id: ColumnType<
      string | null,
      string | null | undefined,
      string | null
    >; /** The host that added this client to the account, when a host did. */
    registered_by: ColumnType<
      string | null,
      string | null | undefined,
      string | null
    >;
  };
  challenges: { nonce: string; user_id: string; expires_at: Date };
  host_sign_ins: {
    code_hash: string;
    user_code: string;
    key: string;
    name: string;
    platform: string;
    channel: string;
    approved_by: string | null;
    requested_from: string | null;
    created_at: CreatedAt;
    expires_at: Date;
  };
  push_tokens: {
    device_key: string;
    platform: string;
    app: string;
    apns_environment: string | null;
    token: string;
    updated_at: ColumnType<Date, never, Date>;
    last_ok_at: ColumnType<Date | null, never, Date | null>;
    failures: ColumnType<number, never, number>;
  };
  updates: {
    id: string;
    platform: string;
    runtime_version: string;
    created_at: Date;
    manifest: Buffer;
    signature: string;
    commit: string;
    message: string;
    published_at: CreatedAt;
  };
  update_channels: {
    update_id: string;
    channel: string;
    assigned_at: CreatedAt;
    withdrawn_at: ColumnType<Date | null, never, Date | null>;
  };
  update_rollbacks: {
    id: Generated<string>;
    update_id: string;
    platform: string;
    runtime_version: string;
    channel: string;
    directive: Buffer;
    signature: string;
    commit_time: Date;
    assigned_at: CreatedAt;
  };
  shares: {
    id: string;
    user_id: string;
    host_key: string;
    created_at: CreatedAt;
    expires_at: Date;
    ended_at: ColumnType<Date | null, never, Date | null>;
    ended_reason: ColumnType<string | null, never, string | null>;
    viewer_check: string | null;
  };
  audit: {
    id: Generated<string>;
    user_id: string | null;
    actor: string;
    action: string;
    subject: string | null;
    detail: ColumnType<unknown, string | undefined, never>;
    at: CreatedAt;
  };
}

export interface Database {
  pool: pg.Pool;
  db: Kysely<Tables>;
  ping(timeoutMs: number): Promise<boolean>;
  close(): Promise<void>;
}

export function openDatabase(url: string, log: Logger): Database {
  const pool = new pg.Pool({
    connectionString: url,
    max: 20,
    idleTimeoutMillis: 30_000,
    connectionTimeoutMillis: 3_000,
    application_name: "sikemux-api",
  });
  pool.on("error", (error) =>
    log.error({ err: error }, "an idle database connection failed"),
  );
  const db = new Kysely<Tables>({ dialect: new PostgresDialect({ pool }) });

  return {
    pool,
    db,
    async ping(timeoutMs) {
      let timer: NodeJS.Timeout | undefined;
      const timeout = new Promise<false>((resolve) => {
        timer = setTimeout(() => resolve(false), timeoutMs);
      });
      const query = sql`select 1`.execute(db).then(
        () => true,
        (error: unknown) => {
          log.warn({ err: error }, "the database did not answer");
          return false;
        },
      );
      try {
        return await Promise.race([query, timeout]);
      } finally {
        clearTimeout(timer);
      }
    },
    async close() {
      await db.destroy();
      if (!pool.ended) await pool.end();
    },
  };
}
