import {
  Kysely,
  PostgresDialect,
  sql,
  type ColumnType,
  type Generated,
} from "kysely";
import type {
  AccountEventType,
  ApnsEnvironment,
  Channel,
  DeviceRole,
  HookHeader,
  HookMethod,
  Platform,
  PushApp,
  PushPlatform,
  RevokeReason,
  ShareEndReason,
  StashKind,
  StashLink,
  StashPreview,
  StashTarget,
} from "@sikemux/protocol";
import pg from "pg";

import type { Logger } from "./log.ts";
import type { UpdateChannel, UpdatePlatform } from "./updates/update.ts";

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
    stash_seen_at: ColumnType<Date | null, never, Date>;
  };
  events: {
    id: Generated<string>;
    user_id: string;
    type: AccountEventType;
    subject: string | null;
    subject_role: DeviceRole | null;
    reason: RevokeReason | null;
    at: CreatedAt;
  };
  removed_devices: {
    key: string;
    user_id: string;
    role: DeviceRole;
    reason: RevokeReason;
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
    role: DeviceRole;
    name: string;
    platform: Platform;
    channel: Channel | null;
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
  host_tokens: {
    token_hash: string;
    user_id: string;
    created_at: CreatedAt;
    expires_at: Date;
  };
  host_sign_ins: {
    code_hash: string;
    user_code: string;
    key: string;
    name: string;
    platform: Platform;
    channel: Channel;
    approved_by: string | null;
    requested_from: string | null;
    created_at: CreatedAt;
    expires_at: Date;
  };
  push_tokens: {
    device_key: string;
    platform: PushPlatform;
    app: PushApp;
    apns_environment: ApnsEnvironment | null;
    token: string;
    updated_at: ColumnType<Date, never, Date>;
    last_ok_at: ColumnType<Date | null, never, Date | null>;
    failures: ColumnType<number, never, number>;
  };
  updates: {
    id: string;
    platform: UpdatePlatform;
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
    channel: UpdateChannel;
    assigned_at: CreatedAt;
    withdrawn_at: ColumnType<Date | null, never, Date | null>;
  };
  update_rollbacks: {
    id: Generated<string>;
    update_id: string;
    platform: UpdatePlatform;
    runtime_version: string;
    channel: UpdateChannel;
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
    ended_reason: ColumnType<
      ShareEndReason | null,
      never,
      ShareEndReason | null
    >;
    viewer_check: string | null;
  };
  stash_items: {
    id: string;
    user_id: string;
    kind: StashKind;
    url: string | null;
    text: string | null;
    note: string | null;
    name: string | null;
    mime: string | null;
    size: ColumnType<string | null, number | null, never>;
    preview: ColumnType<StashPreview | null, string | null, string | null>;
    /** On text and prompts, the links written in its words, each with its page's preview. */
    links: ColumnType<
      StashLink[] | null,
      string | null | undefined,
      string | null
    >;
    target: ColumnType<StashTarget | null, string | null, never>;
    from_device: string | null;
    created_at: CreatedAt;
    taken_at: ColumnType<Date | null, never, Date | null>;
    /** The prompt this file is attached to. */
    prompt_id: ColumnType<string | null, string | null | undefined, never>;
    /** On a prompt, how many of its attachments have yet to arrive; it stays hidden until none. */
    attachments_missing: ColumnType<number, number | undefined, number>;
  };
  hooks: {
    id: string;
    user_id: string;
    host_key: string;
    token_sha256: Buffer;
    created_at: CreatedAt;
  };
  hook_deliveries: {
    id: Generated<string>;
    hook_id: string;
    host_key: string;
    received_at: CreatedAt;
    method: HookMethod;
    headers: ColumnType<HookHeader[], string, never>;
    query: string;
    body: Buffer;
    attempts: ColumnType<number, never, number>;
    retry_at: ColumnType<Date | null, never, Date | null>;
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
