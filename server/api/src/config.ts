import facts from "@sikemux/protocol/release.json" with { type: "json" };
import { isAbsolute } from "node:path";
import type { Network } from "@sikemux/protocol";
import type { Level } from "pino";

import { readAppleSignIn } from "./account/apple.ts";
import { CLERK_API } from "./account/clerk.ts";
import { isSandboxKey, PADDLE_KEY } from "./billing/paddle-api.ts";
import type { AppleKey } from "./apple-key.ts";
import { settingsFrom } from "./env.ts";
import { readJoinSigner, type JoinSigner } from "./join/signer.ts";
import { readNetwork } from "./network/network.ts";
import { readPush, type PushSettings } from "./push/settings.ts";
import { version } from "./version.ts";

interface MigrationConfig {
  databaseUrl: string;
  logLevel: Level;
}

interface PurgeConfig extends MigrationConfig {
  /** Where stashed files are kept, so purging can delete them with their items; null without a stash. */
  stashDir: string | null;
}

/** Where stashed files are kept and the key they are encrypted with. */
interface StashSettings {
  dir: string;
  key: Buffer;
}

export interface Config extends MigrationConfig {
  host: string;
  port: number;
  /** The web app's origin, the only page allowed to call the API from a browser. */
  appOrigin: string;
  /** The API's own public origin, which webhook addresses start with. */
  apiOrigin: string;
  /** The Clerk instance whose sign-ins the API accepts, such as https://clerk.sikemux.com. */
  clerkIssuer: string;
  /** The Mac app's Clerk OAuth client, the only one whose access tokens are accepted. */
  macClientId: string;
  /** Where Clerk's Backend API is: Clerk's own, or the local stand-in sign-in that names its made-up people. */
  clerkApi: string;
  /** For Clerk's Backend API. Without it, deleting accounts and revoking removed phones' sessions wait in the database, and sign-ups go unchecked; every release has one. */
  clerkSecretKey: string | null;
  /** Checks the signatures on Clerk's webhooks. Without it, the webhook route refuses everything. */
  clerkWebhookSecret: string | null;
  /** What GET /v1/network answers: the relay apps use and the oldest app versions allowed. */
  network: Network;
  push: PushSettings;
  /** The key Sign in with Apple takes the API's client secrets with, or null when it is not set up. */
  appleSignIn: AppleKey | null;
  /** The key join tickets are signed with. */
  join: JoinSigner;
  /** Where DB-IP's databases are, to say roughly where a host asking to sign in is; null without them. */
  placesDir: string | null;
  /** What the relay sends to ask whether a device may use it; null leaves the relay's question unanswered. */
  relayAccessToken: string | null;
  /** Null when STASH_DIR and STASH_KEY are not set, and the stash answers that it is not set up. */
  stash: StashSettings | null;
  /** Keys the hash a proven work address is kept as. Null leaves addresses unproven; every release has one. */
  addressProofKey: Buffer | null;
  billing: BillingSettings;
}

/** Which payment provider billing goes through, and whether plans are on sale to everyone. */
type BillingSettings = {
  /** Off, billing answers only staff and nothing a plan pays for is held back from anyone. */
  live: boolean;
} &
  /** Null leaves billing answering that it is not set up. The stand-in never runs in a release. */
  (
    | { provider: "stand-in" | null }
    /** Paddle's sandbox or live account, whichever the key is for. */
    | { provider: "paddle"; paddleKey: string }
  );

function readPaddleKey(env: NodeJS.ProcessEnv, problems: string[]) {
  const key = env.PADDLE_API_KEY?.trim() ?? "";
  if (PADDLE_KEY.test(key)) return key;
  problems.push(
    key
      ? "PADDLE_API_KEY is not a Paddle API key like pdl_sdbx_apikey_…"
      : "BILLING_PROVIDER=paddle needs PADDLE_API_KEY",
  );
  return null;
}

const LEVELS: readonly Level[] = [
  "fatal",
  "error",
  "warn",
  "info",
  "debug",
  "trace",
];

function reader(env: NodeJS.ProcessEnv) {
  const settings = settingsFrom(env, []);
  const { problems, text } = settings;
  const readDatabaseUrl = () => {
    const databaseUrl = text("DATABASE_URL");
    if (databaseUrl && !/^postgres(ql)?:\/\//.test(databaseUrl))
      problems.push("DATABASE_URL is not a postgres:// URL");
    return databaseUrl;
  };
  const readLogLevel = () => settings.oneOf("LOG_LEVEL", LEVELS, "info");
  const readStashDir = () => {
    const dir = settings.optional("STASH_DIR") ?? null;
    if (dir !== null && !isAbsolute(dir))
      problems.push("STASH_DIR is not an absolute path");
    return dir;
  };
  const finish = <T>(config: T): T => {
    if (problems.length)
      throw new Error(`The API cannot start: ${problems.join("; ")}.`);
    return config;
  };
  return { ...settings, readDatabaseUrl, readLogLevel, readStashDir, finish };
}

/** Reads only what migrating needs, so a deploy can migrate without the server's settings. */
export function loadMigrationConfig(env: NodeJS.ProcessEnv): MigrationConfig {
  const { readDatabaseUrl, readLogLevel, finish } = reader(env);
  return finish({ databaseUrl: readDatabaseUrl(), logLevel: readLogLevel() });
}

export function loadPurgeConfig(env: NodeJS.ProcessEnv): PurgeConfig {
  const { readDatabaseUrl, readLogLevel, readStashDir, finish } = reader(env);
  return finish({
    databaseUrl: readDatabaseUrl(),
    logLevel: readLogLevel(),
    stashDir: readStashDir(),
  });
}

/** What the paddle-setup command needs: the key, and the API's origin its webhook is under. */
export function loadPaddleSetupConfig(env: NodeJS.ProcessEnv) {
  const { problems, text, readLogLevel, finish } = reader(env);
  const apiOrigin = text("API_ORIGIN");
  if (apiOrigin && URL.parse(apiOrigin)?.origin !== apiOrigin)
    problems.push("API_ORIGIN is not an origin like https://api.sikemux.com");
  const appOrigin = text("APP_ORIGIN", "https://app.sikemux.com");
  if (appOrigin && URL.parse(appOrigin)?.origin !== appOrigin)
    problems.push("APP_ORIGIN is not an origin like https://app.sikemux.com");
  const paddleKey = readPaddleKey(env, problems) ?? "";
  return finish({ apiOrigin, appOrigin, paddleKey, logLevel: readLogLevel() });
}

/**
 * Reads the configuration from the environment, refusing to start on anything missing or
 * malformed. `release` is whether this is the bundle CI builds, which never makes its own keys.
 */
export function loadConfig(
  env: NodeJS.ProcessEnv,
  { release = version !== "dev" } = {},
): Config {
  const {
    problems,
    text,
    optional,
    oneOf,
    flag,
    port: readPort,
    readDatabaseUrl,
    readLogLevel,
    readStashDir,
    finish,
  } = reader(env);

  const databaseUrl = readDatabaseUrl();

  const port = readPort("PORT", String(facts.accounts.dev.apiPort));

  const appOrigin = text("APP_ORIGIN", "https://app.sikemux.com");
  if (appOrigin && URL.parse(appOrigin)?.origin !== appOrigin)
    problems.push("APP_ORIGIN is not an origin like https://app.sikemux.com");

  const apiOrigin = text("API_ORIGIN", facts.accounts.production.apiUrl);
  if (apiOrigin && URL.parse(apiOrigin)?.origin !== apiOrigin)
    problems.push("API_ORIGIN is not an origin like https://api.sikemux.com");

  const clerkIssuer = text("CLERK_ISSUER");
  const issuerUrl = clerkIssuer ? URL.parse(clerkIssuer) : null;
  const standInIssuer =
    issuerUrl?.protocol === "http:" && issuerUrl.hostname === "127.0.0.1";
  if (
    clerkIssuer &&
    (issuerUrl?.origin !== clerkIssuer ||
      (!clerkIssuer.startsWith("https://") && !standInIssuer))
  )
    problems.push(
      "CLERK_ISSUER is not an https origin like https://clerk.sikemux.com",
    );
  if (standInIssuer && release)
    problems.push("CLERK_ISSUER on 127.0.0.1 is for development only");

  const macClientId = text("CLERK_MAC_CLIENT_ID");

  const clerkSecretKey = optional("CLERK_SECRET_KEY") ?? null;
  if (clerkSecretKey === null && release)
    problems.push("CLERK_SECRET_KEY is not set");
  if (clerkSecretKey && !/^sk_(live|test)_\S+$/.test(clerkSecretKey))
    problems.push("CLERK_SECRET_KEY is not a secret key like sk_live_…");

  const clerkWebhookSecret = optional("CLERK_WEBHOOK_SECRET") ?? null;
  if (clerkWebhookSecret && !/^whsec_[A-Za-z0-9+/=]+$/.test(clerkWebhookSecret))
    problems.push("CLERK_WEBHOOK_SECRET is not a signing secret like whsec_…");

  const network = readNetwork(env, problems);
  const push = readPush(env, problems);
  const appleSignIn = readAppleSignIn(env, problems);
  const join = readJoinSigner(env, problems, release);
  const logLevel = readLogLevel();
  const host = text("HOST", "127.0.0.1");
  const placesDir = optional("GEOIP_DIR") ?? null;
  const relayAccessToken = optional("RELAY_ACCESS_TOKEN") ?? null;
  if (relayAccessToken !== null && relayAccessToken.length < 32)
    problems.push("RELAY_ACCESS_TOKEN is shorter than 32 characters");
  const stashDir = readStashDir();
  const stashKey = optional("STASH_KEY") ?? null;
  if (stashKey !== null && !/^[0-9a-f]{64}$/.test(stashKey))
    problems.push("STASH_KEY is not 32 bytes in lowercase hex");
  if ((stashDir === null) !== (stashKey === null))
    problems.push("STASH_DIR and STASH_KEY are set together or not at all");
  const stash =
    stashDir && stashKey
      ? { dir: stashDir, key: Buffer.from(stashKey, "hex") }
      : null;

  const addressProofKey = optional("ADDRESS_PROOF_KEY") ?? null;
  if (addressProofKey !== null && !/^[0-9a-f]{64}$/.test(addressProofKey))
    problems.push("ADDRESS_PROOF_KEY is not 32 bytes in lowercase hex");
  if (addressProofKey === null && release)
    problems.push("ADDRESS_PROOF_KEY is not set");

  const provider = oneOf(
    "BILLING_PROVIDER",
    ["none", "stand-in", "paddle"],
    "none",
  );
  if (provider === "stand-in" && release)
    problems.push("BILLING_PROVIDER=stand-in is for development only");
  const live = flag("BILLING_LIVE", false);
  const paddleKey = provider === "paddle" ? readPaddleKey(env, problems) : null;
  if (live && paddleKey && isSandboxKey(paddleKey))
    problems.push(
      "BILLING_LIVE=true needs a live Paddle key, not a sandbox one",
    );
  const billing: BillingSettings =
    provider === "paddle"
      ? { provider, live, paddleKey: paddleKey ?? "" }
      : { provider: provider === "none" ? null : provider, live };

  return finish({
    host,
    port,
    databaseUrl,
    appOrigin,
    apiOrigin,
    clerkIssuer,
    macClientId,
    clerkApi: standInIssuer ? clerkIssuer : CLERK_API,
    clerkSecretKey,
    clerkWebhookSecret,
    network,
    push,
    appleSignIn,
    join: join as JoinSigner,
    placesDir,
    relayAccessToken,
    stash,
    addressProofKey: addressProofKey
      ? Buffer.from(addressProofKey, "hex")
      : null,
    billing,
    logLevel,
  });
}
