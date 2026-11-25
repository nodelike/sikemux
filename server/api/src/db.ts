import {
  Kysely,
  PostgresDialect,
  sql,
  type ColumnType,
  type Generated,
} from "kysely";
import type {
  AccountEventType,
  BillingInterval,
  BillingProductId,
  InvoiceLine,
  InvoiceReason,
  InvoiceStatus,
  SubscriptionStatus,
  ApnsEnvironment,
  Channel,
  FollowedChannel,
  DeviceRole,
  HookHeader,
  HookMethod,
  OrgDomainJoining,
  OrgRuleAdding,
  OrgRole,
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

export type FoldWhy =
  "invite" | "share" | "browser" | "domain" | "address" | "seat";

/** The tables the API reads and writes. Each one arrives with the migration that creates it. */
export type OrgLogoMime = "image/png" | "image/jpeg" | "image/webp";

export interface Tables {
  users: {
    id: string;
    created_at: CreatedAt;
    deleted_at: ColumnType<Date | null, never, Date | null>;
    clerk_deleted_at: ColumnType<Date | null, never, Date | null>;
    clerk_attempts: ColumnType<number, never, number>;
    clerk_retry_at: ColumnType<Date | null, never, Date | null>;
    purge_after: ColumnType<Date | null, never, Date | null>;
    billing_attempts: ColumnType<number, never, number>;
    billing_retry_at: ColumnType<Date | null, never, Date | null>;
    events_pruned_through: ColumnType<string, never, string | number>;
    stash_seen_at: ColumnType<Date | null, never, Date>;
    staff: ColumnType<boolean, never, boolean>;
    admitted_at: ColumnType<Date | null, Date | undefined, Date>;
    frozen_at: ColumnType<Date | null, never, Date | null>;
    new_kept_at: ColumnType<Date | null, never, Date>;
    strip_attempts: ColumnType<number, never, number>;
    strip_retry_at: ColumnType<Date | null, never, Date | null>;
  };
  address_proofs: {
    id: Generated<string>;
    address_hash: Buffer;
    user_id: string;
    domain: string;
    org_id: string | null;
    via: "fold" | "google";
    proven_at: ColumnType<Date, never, never>;
  };
  account_folds: {
    id: Generated<string>;
    from_user: ColumnType<string | null, string | undefined, string>;
    into_user: ColumnType<string | null, string | undefined, string>;
    secret_hash: Buffer;
    why: FoldWhy;
    org_id: string | null;
    address_hash: ColumnType<Buffer | null, Buffer | undefined, Buffer>;
    domain: ColumnType<string | null, string | undefined, string>;
    created_at: CreatedAt;
    expires_at: Date;
    done_at: ColumnType<Date | null, never, Date>;
    started_session: ColumnType<
      string | null,
      string | null | undefined,
      never
    >;
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
  work_address_sittings: {
    id: Generated<string>;
    user_id: string;
    state_hash: Buffer;
    nonce: string;
    verifier: string;
    address_hash: Buffer | null;
    created_at: CreatedAt;
    proven_at: Date | null;
    joined_at: Date | null;
  };
  sign_in_removals: {
    id: Generated<string>;
    user_id: string;
    address_id: string;
    domain: string;
    org_name: string | null;
    provider: string | null;
    removed_at: CreatedAt;
    told_at: Date | null;
    end_sessions_from: Date | null;
    swept_from: Date | null;
    address_seen_id: string | null;
    rerun_attempts: Generated<number>;
    rerun_at: Generated<Date>;
  };
  addresses_seen: {
    id: Generated<string>;
    user_id: string;
    address_hash: Buffer;
    domain_hash: Buffer;
    first_seen: Date;
    ended_at: Date | null;
    could_sign_in: Generated<boolean>;
  };
  host_removal_sign_outs: {
    id: Generated<string>;
    user_id: string;
    kept_session: string | null;
    removed_at: CreatedAt;
    clerk_attempts: Generated<number>;
    clerk_retry_at: Generated<Date>;
    done_at: Date | null;
  };
  ended_sessions: {
    session_id: string;
    user_id: string;
    made_at: Date;
    ended_at: ColumnType<Date | null, Date | null | undefined, Date>;
    clerk_attempts: Generated<number>;
    clerk_retry_at: Generated<Date>;
    cause: Generated<"work_address" | "device_removed">;
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
    /** The releases the host last said it follows, and when. */
    followed_channel: ColumnType<
      FollowedChannel | null,
      never,
      FollowedChannel | null
    >;
    followed_at: ColumnType<Date | null, never, Date | null>;
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
    /** Its join tickets say it never signs for the person. */
    hands_only: ColumnType<boolean, boolean | undefined, boolean>;
    sign_in_session: ColumnType<
      string | null,
      string | null | undefined,
      string | null
    >;
    mac_signed_in_at: ColumnType<
      Date | null,
      Date | null | undefined,
      Date | null
    >;
  };
  challenges: {
    nonce: string;
    user_id: string;
    expires_at: Date;
    mac_signed_in_at: Date | null;
  };
  host_sign_ins: {
    code_hash: string;
    user_code: string;
    key: string;
    name: string;
    platform: Platform;
    channel: Channel;
    approved_by: string | null;
    approved_session: ColumnType<
      string | null,
      string | null | undefined,
      string | null
    >;
    approved_mac_signed_in_at: ColumnType<
      Date | null,
      Date | null | undefined,
      Date | null
    >;
    approved_by_host: Generated<boolean>;
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
    anyone: ColumnType<boolean, boolean | undefined, boolean>;
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
  vaults: {
    user_id: string;
    generation: ColumnType<number, never, number>;
    recovery_key: string;
    recovery_wrap: string;
    rotate: ColumnType<boolean, never, boolean>;
    created_by: string;
    created_at: CreatedAt;
    rotated_at: ColumnType<Date | null, never, Date>;
  };
  vault_keys: {
    user_id: string;
    host_key: string;
    generation: number;
    wrapped: string;
    wrapped_by: string;
    created_at: ColumnType<Date, never, Date>;
  };
  vault_entries: {
    id: string;
    user_id: string;
    plugin: string;
    generation: number;
    sealed: string;
    revision: number;
    updated_by: string;
    updated_at: ColumnType<Date, never, Date>;
    created_at: CreatedAt;
  };
  vault_ended: {
    user_id: string;
    id: string;
    revision: number;
  };
  org_vaults: {
    org_id: string;
    generation: ColumnType<number, never, number>;
    created_by: string;
    created_at: CreatedAt;
    rotated_at: ColumnType<Date | null, never, Date>;
    remade_at: ColumnType<Date | null, never, Date>;
    changes: ColumnType<string, never, never>;
  };
  org_vault_keys: {
    org_id: string;
    host_key: string;
    generation: number;
    wrapped: string;
    wrapped_by: string;
    created_at: ColumnType<Date, never, Date>;
  };
  org_vault_vouches: {
    org_id: string;
    host_key: string;
    member: string;
    by_key: string;
    by_member: string;
    at: ColumnType<string, number, number>;
    signature: string;
    introduction: string | null;
  };
  org_vault_entries: {
    id: string;
    org_id: string;
    kind: string;
    generation: number;
    sealed: string;
    revision: number;
    updated_by: string;
    updated_at: ColumnType<Date, never, Date>;
    created_at: CreatedAt;
    change: ColumnType<string, never, never>;
  };
  org_vault_ended: {
    org_id: string;
    id: string;
    revision: number;
    change: ColumnType<string, never, never>;
  };
  vault_uses: {
    id: Generated<string>;
    user_id: string;
    entry_id: string;
    plugin: string;
    host_key: string;
    actor: string;
    count: number;
    first_at: ColumnType<Date, string, string>;
    last_at: ColumnType<Date, string, string>;
  };
  mod_counters: {
    user_id: string;
    seq: ColumnType<string, never, number>;
    sealed_bytes: ColumnType<string, never, number>;
    cleared_seq: ColumnType<string, never, never>;
    file_bytes: ColumnType<string, never, number>;
  };
  mod_files: {
    user_id: string;
    mod_id: string;
    file_id: string;
    stored_as: string;
    size: ColumnType<string, number, never>;
    stored_by: string;
    stored_at: CreatedAt;
  };
  pages: {
    id: string;
    user_id: string;
    stored_as: string | null;
    size: ColumnType<string, number, number>;
    viewer_check: string;
    anyone: boolean;
    views: ColumnType<number, never, number>;
    expires_at: ColumnType<Date, Date, Date>;
    ended_at: ColumnType<Date | null, never, Date | null>;
    stored_by: string;
    created_at: CreatedAt;
    updated_at: ColumnType<Date, never, Date>;
  };
  mod_documents: {
    user_id: string;
    mod_id: string;
    doc_id: string;
    generation: number;
    sealed: string;
    removed: boolean;
    revision: number;
    seq: ColumnType<string, number, number>;
    updated_by: string;
    updated_at: ColumnType<Date, never, Date>;
  };
  mod_purges: {
    user_id: string;
    seq: ColumnType<string, number, never>;
    mod_id: string;
    purged_by: string;
    purged_at: CreatedAt;
  };
  orgs: {
    id: Generated<string>;
    name: string;
    created_by: string;
    created_at: CreatedAt;
    rule_adding: Generated<OrgRuleAdding>;
    asking: Generated<boolean>;
    daily_questions: Generated<number>;
    preview_seats: ColumnType<
      number | null,
      number | null | undefined,
      number | null
    >;
  };
  team_audience: {
    only_row: Generated<boolean>;
    audience: Generated<"staff" | "nightly" | "everyone">;
    vaults_open: Generated<boolean>;
  };
  org_users: {
    id: Generated<string>;
    org_id: string;
    user_id: string;
    role: OrgRole;
    invited_by: string | null;
    joined_at: CreatedAt;
  };
  org_shares: {
    member_id: string;
    user_id: string;
    project: string;
    host: string;
    tapped_at: ColumnType<Date, Date | undefined, Date>;
    created_at: CreatedAt;
    updated_at: ColumnType<Date, never, Date>;
  };
  org_invites: {
    id: Generated<string>;
    org_id: string;
    role: OrgRole;
    token_hash: string;
    invited_by: string;
    created_at: CreatedAt;
    expires_at: Date;
    accepted_by: ColumnType<string | null, never, string>;
    accepted_at: ColumnType<Date | null, never, Date>;
    revoked_at: ColumnType<Date | null, never, Date>;
  };
  org_teams: {
    id: Generated<string>;
    org_id: string;
    name: string;
    created_at: CreatedAt;
  };
  org_team_members: {
    team_id: string;
    member_id: string;
    added_at: CreatedAt;
  };
  org_logos: {
    org_id: string;
    id: Generated<string>;
    mime: OrgLogoMime;
    bytes: Buffer;
  };
  org_domains: {
    id: Generated<string>;
    org_id: string;
    domain: string;
    token: string;
    joining: OrgDomainJoining;
    added_by: string;
    created_at: CreatedAt;
    verified_at: Date | null;
    ours_since: Date | null;
  };
  billing_customers: {
    id: Generated<string>;
    user_id: string | null;
    org_id: string | null;
    provider: string;
    provider_customer_id: string | null;
    paid_by: ColumnType<string | null, never, string | null>;
    created_at: CreatedAt;
  };
  billing_people: {
    provider: string;
    user_id: string;
    provider_customer_id: string;
    created_at: CreatedAt;
  };
  billing_checkouts: {
    provider: string;
    provider_checkout_id: string;
    customer_id: string;
    opened_by: string;
    provider_customer_id: string;
    product: BillingProductId;
    interval: BillingInterval;
    quantity: number;
    created_at: CreatedAt;
  };
  subscriptions: {
    id: Generated<string>;
    customer_id: string;
    provider_subscription_id: string;
    product: BillingProductId;
    interval: BillingInterval;
    quantity: number;
    status: SubscriptionStatus | "paused";
    current_period_end: Date;
    cancel_at_period_end: boolean;
    started_at: Date;
    ended_at: Date | null;
    next_product: BillingProductId | null;
    next_interval: BillingInterval | null;
    next_quantity: number | null;
    provider_updated_at: Date;
    seats_cut_to: Generated<number | null>;
    seats_cut_at: Generated<Date | null>;
    paused_for_seat_at: Generated<Date | null>;
    seat_pause_asked_for: Generated<Date | null>;
    cancel_asked_at: Generated<Date | null>;
    kept_without_refund_at: Generated<Date | null>;
    resume_asked_at: Generated<Date | null>;
    resume_asked_for: Generated<Date | null>;
    /** An interval, only ever reckoned with in SQL. */
    paid_left: Generated<string | null>;
    resumes_at: Generated<Date | null>;
  };
  invoice_refunds: {
    provider_refund_id: string;
    invoice_id: string;
    amount: ColumnType<string, number, never>;
    at: Date;
  };
  invoices: {
    id: Generated<string>;
    customer_id: string;
    provider_subscription_id: string;
    provider_payment_id: string;
    lines: ColumnType<InvoiceLine[], string, never>;
    reason: InvoiceReason;
    amount: ColumnType<string, number, number>;
    currency: string;
    status: InvoiceStatus;
    at: Date;
    refund_requested_at: Date | null;
    refunded_at: Date | null;
    kept_refund: ColumnType<
      string | null,
      number | null | undefined,
      number | null
    >;
    kept_refunded_at: Generated<Date | null>;
    kept_refund_refused_at: Generated<Date | null>;
    kept_refund_check_at: Generated<Date | null>;
  };
  billing_events: {
    provider: string;
    event_id: string;
    type: string;
    received_at: CreatedAt;
  };
  billing_strays: {
    provider: string;
    provider_id: string;
    created_at: CreatedAt;
  };
  plan_gifts: {
    user_id: string;
    plan: "pro";
    given_at: CreatedAt;
    ends_at: Date;
  };
  billing_held_sales: {
    provider: string;
    provider_subscription_id: string;
    provider_checkout_id: string;
    created_at: CreatedAt;
  };
  billing_calls: {
    id: Generated<string>;
    provider: string;
    action:
      | "cancel_now"
      | "cancel_at_period_end"
      | "keep_cancel"
      | "refund"
      | "settle_seat_pause";
    subscription: ColumnType<
      Record<string, unknown> | null,
      string | null,
      never
    >;
    provider_payment_id: string | null;
    user_id: string | null;
    refund_amount: ColumnType<string | null, number | null, never>;
    attempts: ColumnType<number, never, number>;
    retry_at: ColumnType<Date, never, Date>;
    created_at: CreatedAt;
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
