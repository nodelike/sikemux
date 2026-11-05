import facts from "@sikemux/protocol/release.json" with { type: "json" };
import { isAbsolute } from "node:path";
import type { Network } from "@sikemux/protocol";
import type { Level } from "pino";

import { readAppleSignIn } from "./account/apple.ts";
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
  /** For Clerk's Backend API. Without it, deleting accounts and revoking removed phones' sessions wait in the database. */
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

  const apiOrigin = text("API_ORIGIN", "https://api.sikemux.com");
  if (apiOrigin && URL.parse(apiOrigin)?.origin !== apiOrigin)
    problems.push("API_ORIGIN is not an origin like https://api.sikemux.com");

  const clerkIssuer = text("CLERK_ISSUER");
  if (
    clerkIssuer &&
    (URL.parse(clerkIssuer)?.origin !== clerkIssuer ||
      !clerkIssuer.startsWith("https://"))
  )
    problems.push(
      "CLERK_ISSUER is not an https origin like https://clerk.sikemux.com",
    );

  const macClientId = text("CLERK_MAC_CLIENT_ID");

  const clerkSecretKey = optional("CLERK_SECRET_KEY") ?? null;
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

  return finish({
    host,
    port,
    databaseUrl,
    appOrigin,
    apiOrigin,
    clerkIssuer,
    macClientId,
    clerkSecretKey,
    clerkWebhookSecret,
    network,
    push,
    appleSignIn,
    join: join as JoinSigner,
    placesDir,
    relayAccessToken,
    stash,
    logLevel,
  });
}
