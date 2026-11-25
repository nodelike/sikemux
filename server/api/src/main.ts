import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

import type { Level } from "pino";

import { purgeAccounts, pruneHistory } from "./account/purge.ts";
import { PaddleApi } from "./billing/paddle-api.ts";
import { setUpPaddle } from "./billing/paddle-setup.ts";
import {
  loadConfig,
  loadMigrationConfig,
  loadPaddleSetupConfig,
  loadPurgeConfig,
} from "./config.ts";
import { openDatabase, type Database } from "./db.ts";
import { createLogger } from "./log.ts";
import { migrate, readMigrations } from "./migrations.ts";
import { flushSentry, startSentry } from "./sentry.ts";
import { startServer } from "./server.ts";
import { checkStashFiles, StashFiles } from "./stash/files.ts";
import { purgeHookDeliveries } from "./hooks/purge.ts";
import { purgePages } from "./pages/store.ts";
import { purgeStash } from "./stash/purge.ts";
import { purgeVaultUses } from "./vault/purge.ts";
import { purgeModRemovals } from "./mods/purge.ts";
import { endPlanGifts, givePro } from "./billing/gifts.ts";
import {
  setTeamAudience,
  setTeamVaults,
  TEAM_AUDIENCES,
  TEAM_VAULTS,
} from "./billing/team-audience.ts";
import {
  promoteUpdate,
  publishUpdate,
  UpdateRefused,
  withdrawUpdate,
} from "./updates/publish.ts";

const updatesCertificate = () =>
  readFileSync(new URL("./updates-certificate.pem", import.meta.url), "utf8");

function startLogging(level: Level) {
  const log = createLogger(level);
  process.on("unhandledRejection", (error) => {
    log.fatal({ err: error }, "an unhandled rejection");
    void flushSentry().finally(() => process.exit(1));
  });
  return log;
}

type Log = ReturnType<typeof startLogging>;

/** Opens the database for a command that runs and exits: any failure is logged and fails the process. */
async function withDatabase(
  { databaseUrl, logLevel }: { databaseUrl: string; logLevel: Level },
  failed: string,
  run: (database: Database, log: Log) => Promise<void>,
) {
  const log = startLogging(logLevel);
  const database = openDatabase(databaseUrl, log);
  try {
    await run(database, log);
  } catch (error) {
    if (error instanceof UpdateRefused) log.error(error.message);
    else log.fatal({ err: error }, failed);
    process.exitCode = 1;
  } finally {
    await database.close();
  }
}

const updateCommand =
  (
    name: string,
    run: (
      database: Database,
      log: Log,
      first?: string,
      second?: string,
    ) => Promise<void>,
  ) =>
  async () => {
    const [first, second] = process.argv.slice(3);
    await withDatabase(
      loadMigrationConfig(process.env),
      `${name} failed`,
      (database, log) => run(database, log, first, second),
    );
  };

const COMMANDS: Record<string, () => void | Promise<void>> = {
  async serve() {
    const config = loadConfig(process.env);
    await startServer(config, startLogging(config.logLevel));
  },
  migrate: () =>
    withDatabase(
      loadMigrationConfig(process.env),
      "migrating failed",
      async (database, log) => {
        const migrations = await readMigrations(
          fileURLToPath(new URL("../migrations", import.meta.url)),
        );
        const applied = await migrate(database.pool, migrations, log);
        log.info(
          { applied: applied.length, total: migrations.length },
          "the database is up to date",
        );
      },
    ),
  purge() {
    const config = loadPurgeConfig(process.env);
    return withDatabase(config, "purging failed", async (database, log) => {
      await purgeAccounts(database.db, log);
      await pruneHistory(database.db, log);
      await purgePages(database.db, config.stashDir, log);
      await purgeStash(database.db, config.stashDir, log);
      await purgeHookDeliveries(database.db, log);
      await purgeVaultUses(database.db, log);
      await purgeModRemovals(database.db, log);
      await endPlanGifts(database.db, log);
    });
  },
  "give-pro"() {
    const days = Number(process.argv[3]);
    if (!Number.isInteger(days) || days < 1) {
      startLogging("info").fatal("usage: give-pro <days>");
      process.exitCode = 2;
      return;
    }
    return withDatabase(
      loadMigrationConfig(process.env),
      "giving Pro failed",
      async (database, log) => {
        await givePro(database.db, days, log);
      },
    );
  },
  "team-audience"() {
    const audience = TEAM_AUDIENCES.find((known) => known === process.argv[3]);
    if (!audience) {
      startLogging("info").fatal(
        `usage: team-audience <${TEAM_AUDIENCES.join("|")}>`,
      );
      process.exitCode = 2;
      return;
    }
    return withDatabase(
      loadMigrationConfig(process.env),
      "setting who Team is open to failed",
      ({ db }, log) => setTeamAudience(db, audience, log),
    );
  },
  "team-vaults"() {
    const vaults = TEAM_VAULTS.find((known) => known === process.argv[3]);
    if (!vaults) {
      startLogging("info").fatal(
        `usage: team-vaults <${TEAM_VAULTS.join("|")}>`,
      );
      process.exitCode = 2;
      return;
    }
    return withDatabase(
      loadMigrationConfig(process.env),
      "setting whether orgs' vaults are open failed",
      ({ db }, log) => setTeamVaults(db, vaults, log),
    );
  },
  "publish-update": updateCommand(
    "publish-update",
    async (database, log, first, second) => {
      if (!first || !second)
        throw new UpdateRefused(
          "usage: publish-update <unpacked bundle> <asset folder>",
        );
      const published = await publishUpdate(database.db, {
        dir: resolve(first),
        assetsDir: resolve(second),
        certificate: updatesCertificate(),
      });
      log.info(
        published,
        published.added
          ? "published an update"
          : "the update was already published",
      );
    },
  ),
  "withdraw-update": updateCommand(
    "withdraw-update",
    async (database, log, first) => {
      if (!first) throw new UpdateRefused("usage: withdraw-update <update id>");
      const withdrawn = await withdrawUpdate(database.db, first);
      log.info(
        withdrawn,
        withdrawn.channels.length > 0
          ? "withdrew an update"
          : "the update was already withdrawn",
      );
    },
  ),
  "promote-update": updateCommand(
    "promote-update",
    async (database, log, first) => {
      if (!first) throw new UpdateRefused("usage: promote-update <update id>");
      const promoted = await promoteUpdate(database.db, first);
      log.info(
        promoted,
        promoted.added
          ? "promoted an update to stable"
          : "the update is already on stable",
      );
    },
  ),
  async "paddle-setup"() {
    const config = loadPaddleSetupConfig(process.env);
    const log = startLogging(config.logLevel);
    try {
      await setUpPaddle(new PaddleApi(config.paddleKey, log), config, log);
    } catch (error) {
      log.fatal({ err: error }, "setting up Paddle failed");
      process.exitCode = 1;
    }
  },
  async "check-stash"() {
    const log = startLogging("info");
    const [dir, ...ids] = process.argv.slice(3);
    const key = process.env.STASH_KEY?.trim() ?? "";
    if (!dir || !/^[0-9a-f]{64}$/.test(key)) {
      log.fatal("usage: STASH_KEY=<hex> check-stash <folder> <item id>...");
      process.exitCode = 2;
    } else {
      const files = new StashFiles({ dir, key: Buffer.from(key, "hex") });
      if (await checkStashFiles(files, ids, log))
        log.info({ files: ids.length }, "the stashed files decrypt");
      else process.exitCode = 1;
    }
  },
};

startSentry(process.env);
const command = process.argv[2] ?? "serve";
const run = COMMANDS[command];
if (run) {
  await run();
  if (command !== "serve") await flushSentry();
} else {
  startLogging("info").fatal(
    { command },
    `unknown command; use ${Object.keys(COMMANDS).join(", ")}`,
  );
  process.exitCode = 2;
}
