import { createHash, randomBytes, verify, X509Certificate } from "node:crypto";
import {
  chmod,
  link,
  lstat,
  readdir,
  readFile,
  unlink,
  writeFile,
} from "node:fs/promises";
import { join } from "node:path";
import type { Kysely, Transaction } from "kysely";

import { audit } from "../audit.ts";
import type { Tables } from "../db.ts";
import {
  ASSET_URL,
  isChannel,
  isPlatform,
  RUNTIME_VERSION,
  UPDATE_ID,
  type UpdateChannel,
  type UpdatePlatform,
} from "./update.ts";

/** Refused publish, promote, withdrawal or rollback, with a reason meant for whoever ran it. */
export class UpdateRefused extends Error {}

function refuse(message: string): never {
  throw new UpdateRefused(message);
}

interface UpdateInfo {
  id: string;
  createdAt: string;
  platform: UpdatePlatform;
  runtimeVersion: string;
  channel: UpdateChannel;
  commit: string;
  message: string;
}

interface Asset {
  hex: string;
  contentType: string;
}

const SHA256_HEX = /^[0-9a-f]{64}$/;
const ASSET_FILE = /^[0-9a-f]{64}(\.type)?$/;
const CONTENT_TYPE = /^[a-z]+\/[a-z0-9][a-z0-9.+-]{0,126}$/;
const ASSET_KEY = /^[A-Za-z0-9_-]{1,128}$/;
const BASE64 = /^[A-Za-z0-9+/]+={0,2}$/;
const ISO_INSTANT = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{1,3})?Z$/;
const COMMIT_TIME = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/;
const CLOCK_SKEW_MS = 5 * 60 * 1000;
const BUNDLE_FILES = ["assets", "manifest.json", "manifest.sig", "update.json"];

const sha256 = (bytes: Buffer) => createHash("sha256").update(bytes);

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function readJson(bytes: Buffer, name: string): Record<string, unknown> {
  let value: unknown;
  try {
    value = JSON.parse(bytes.toString("utf8"));
  } catch {
    return refuse(`${name} is not JSON.`);
  }
  return isObject(value) ? value : refuse(`${name} is not a JSON object.`);
}

function readUpdateInfo(value: Record<string, unknown>): UpdateInfo {
  const { id, createdAt, platform, runtimeVersion, channel, commit, message } =
    value;
  if (typeof id !== "string" || !UPDATE_ID.test(id))
    refuse("update.json: id is not a lowercase version 4 UUID.");
  if (
    typeof createdAt !== "string" ||
    !ISO_INSTANT.test(createdAt) ||
    Number.isNaN(Date.parse(createdAt))
  )
    refuse("update.json: createdAt is not an ISO 8601 UTC time.");
  if (!isPlatform(platform))
    refuse("update.json: platform is not android or ios.");
  if (
    typeof runtimeVersion !== "string" ||
    !RUNTIME_VERSION.test(runtimeVersion)
  )
    refuse("update.json: runtimeVersion is missing or malformed.");
  if (!isChannel(channel))
    refuse("update.json: channel is not nightly or stable.");
  if (typeof commit !== "string" || !/^[0-9a-f]{40}$/.test(commit))
    refuse("update.json: commit is not a full git commit.");
  if (typeof message !== "string" || message.length > 2000)
    refuse("update.json: message is not a string of at most 2000 characters.");
  return { id, createdAt, platform, runtimeVersion, channel, commit, message };
}

function readAsset(value: unknown, where: string): Asset {
  if (!isObject(value))
    return refuse(`manifest.json: ${where} is not an object.`);
  const { hash, key, contentType, url } = value;
  if (typeof url !== "string" || !url.startsWith(ASSET_URL))
    refuse(`manifest.json: ${where}.url is not under ${ASSET_URL}.`);
  const hex = url.slice(ASSET_URL.length);
  if (!SHA256_HEX.test(hex))
    refuse(`manifest.json: ${where}.url does not end in a SHA-256.`);
  if (hash !== Buffer.from(hex, "hex").toString("base64url"))
    refuse(`manifest.json: ${where}.hash is not the SHA-256 its url names.`);
  if (typeof key !== "string" || !ASSET_KEY.test(key))
    refuse(`manifest.json: ${where}.key is missing or malformed.`);
  if (typeof contentType !== "string" || !CONTENT_TYPE.test(contentType))
    refuse(`manifest.json: ${where}.contentType is not a plain media type.`);
  return { hex, contentType };
}

function readManifest(bytes: Buffer, update: UpdateInfo): Asset[] {
  const manifest = readJson(bytes, "manifest.json");
  if (manifest.id !== update.id)
    refuse("manifest.json and update.json disagree on id.");
  if (manifest.createdAt !== update.createdAt)
    refuse("manifest.json and update.json disagree on createdAt.");
  if (manifest.runtimeVersion !== update.runtimeVersion)
    refuse("manifest.json and update.json disagree on runtimeVersion.");
  const extra = isObject(manifest.extra) ? manifest.extra : {};
  if (extra.commit !== update.commit)
    refuse("manifest.json and update.json disagree on commit.");
  if (!Array.isArray(manifest.assets))
    refuse("manifest.json: assets is not a list.");
  return [
    readAsset(manifest.launchAsset, "launchAsset"),
    ...manifest.assets.map((asset, index) =>
      readAsset(asset, `assets[${index}]`),
    ),
  ];
}

async function regularFile(path: string, name: string): Promise<Buffer> {
  const stat = await lstat(path).catch(() => undefined);
  if (!stat?.isFile()) refuse(`The bundle has no file ${name}.`);
  return readFile(path);
}

/** Checks the bundle's files are only the ones it may hold, so nothing else can ride along. */
async function checkLayout(dir: string, assets: Asset[]) {
  const top = (await readdir(dir)).sort();
  if (top.join() !== BUNDLE_FILES.join())
    refuse(
      `The bundle holds ${top.join(", ") || "nothing"}; it must hold exactly ${BUNDLE_FILES.join(", ")}.`,
    );
  const assetsStat = await lstat(join(dir, "assets"));
  if (!assetsStat.isDirectory()) refuse("The bundle's assets is not a folder.");
  const named = new Set(assets.map((asset) => asset.hex));
  for (const file of await readdir(join(dir, "assets"))) {
    if (!ASSET_FILE.test(file) || !named.has(file.slice(0, 64)))
      refuse(`assets/${file} is not named by the manifest.`);
  }
}

async function readAssets(dir: string, assets: Asset[]) {
  const files = new Map<string, { bytes: Buffer; contentType: string }>();
  for (const { hex, contentType } of assets) {
    const known = files.get(hex);
    if (known) {
      if (known.contentType !== contentType)
        refuse(`The manifest gives asset ${hex} two content types.`);
      continue;
    }
    const bytes = await regularFile(join(dir, "assets", hex), `assets/${hex}`);
    if (sha256(bytes).digest("hex") !== hex)
      refuse(`assets/${hex} does not match its SHA-256.`);
    const stored = (
      await regularFile(
        join(dir, "assets", `${hex}.type`),
        `assets/${hex}.type`,
      )
    )
      .toString("utf8")
      .trim();
    if (stored !== contentType)
      refuse(
        `assets/${hex}.type says ${stored}, but the manifest says ${contentType}.`,
      );
    files.set(hex, { bytes, contentType });
  }
  return files;
}

function checkSignature(
  bytes: Buffer,
  signature: string,
  certificate: string,
  [name, signatureName]: [string, string],
) {
  if (!BASE64.test(signature)) refuse(`${signatureName} is not base64.`);
  const { publicKey } = new X509Certificate(certificate);
  if (publicKey.asymmetricKeyType !== "rsa")
    refuse("The update certificate does not hold an RSA key.");
  if (!verify("sha256", bytes, publicKey, Buffer.from(signature, "base64")))
    refuse(`${signatureName} is not a signature of ${name} by the update key.`);
}

/**
 * The name an asset is stored under: its SHA-256, then its media type with the slash as a dot,
 * which Caddy reads back into the Content-Type it serves the file with.
 */
export function assetFileName(hex: string, contentType: string): string {
  return `${hex}.${contentType.replace("/", ".")}`;
}

/** Adds a file only if the name is free, never leaving a half-written one under it. */
async function addFile(dir: string, name: string, bytes: Buffer) {
  if (await lstat(join(dir, name)).catch(() => undefined)) return false;
  const partial = join(dir, `.partial-${randomBytes(8).toString("hex")}`);
  await writeFile(partial, bytes);
  await chmod(partial, 0o644);
  try {
    await link(partial, join(dir, name));
    return true;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "EEXIST") return false;
    throw error;
  } finally {
    await unlink(partial);
  }
}

export interface Published {
  id: string;
  platform: UpdatePlatform;
  runtimeVersion: string;
  channel: UpdateChannel;
  added: boolean;
  assetsAdded: number;
}

/**
 * Publishes the update in an unpacked bundle: checks every byte the phone will trust against the
 * signature and the hashes, stores the assets where Caddy serves them, then records the update so
 * the manifest route starts offering it. Publishing the same bundle again changes nothing.
 */
export async function publishUpdate(
  db: Kysely<Tables>,
  {
    dir,
    assetsDir,
    certificate,
  }: { dir: string; assetsDir: string; certificate: string },
): Promise<Published> {
  const update = readUpdateInfo(
    readJson(
      await regularFile(join(dir, "update.json"), "update.json"),
      "update.json",
    ),
  );
  const manifest = await regularFile(
    join(dir, "manifest.json"),
    "manifest.json",
  );
  const signature = (
    await regularFile(join(dir, "manifest.sig"), "manifest.sig")
  )
    .toString("utf8")
    .trim();
  const assets = readManifest(manifest, update);
  await checkLayout(dir, assets);
  checkSignature(manifest, signature, certificate, [
    "manifest.json",
    "manifest.sig",
  ]);
  const files = await readAssets(dir, assets);

  const existing = await db
    .selectFrom("updates")
    .select(["manifest", "signature", "platform"])
    .where("id", "=", update.id)
    .executeTakeFirst();
  if (
    existing &&
    (!existing.manifest.equals(manifest) ||
      existing.platform !== update.platform)
  )
    refuse(
      `An update with id ${update.id} was already published with other contents.`,
    );

  let assetsAdded = 0;
  for (const [hex, { bytes, contentType }] of files) {
    if (await addFile(assetsDir, assetFileName(hex, contentType), bytes))
      assetsAdded += 1;
  }

  const added = await db.transaction().execute(async (trx) => {
    await trx
      .insertInto("updates")
      .values({
        id: update.id,
        platform: update.platform,
        runtime_version: update.runtimeVersion,
        created_at: new Date(update.createdAt),
        manifest,
        signature,
        commit: update.commit,
        message: update.message,
      })
      .onConflict((oc) => oc.column("id").doNothing())
      .execute();
    const assigned = await trx
      .selectFrom("update_channels")
      .select("withdrawn_at")
      .where("update_id", "=", update.id)
      .where("channel", "=", update.channel)
      .executeTakeFirst();
    if (assigned?.withdrawn_at)
      refuse(`${update.id} was withdrawn; publish a new update instead.`);
    if (assigned) return false;
    await checkChannelTakes(
      trx,
      {
        id: update.id,
        platform: update.platform,
        runtime_version: update.runtimeVersion,
        created_at: new Date(update.createdAt),
      },
      update.channel,
    );
    await trx
      .insertInto("update_channels")
      .values({ update_id: update.id, channel: update.channel })
      .execute();
    await audit(trx, {
      userId: null,
      actor: "deploy",
      action: "update.published",
      subject: update.id,
      detail: {
        platform: update.platform,
        runtimeVersion: update.runtimeVersion,
        channel: update.channel,
        commit: update.commit,
      },
    });
    return true;
  });

  return {
    id: update.id,
    platform: update.platform,
    runtimeVersion: update.runtimeVersion,
    channel: update.channel,
    added,
    assetsAdded,
  };
}

export interface Promoted {
  id: string;
  platform: string;
  runtimeVersion: string;
  added: boolean;
}

interface StoredUpdate {
  id: string;
  platform: string;
  runtime_version: string;
  created_at: Date;
}

/**
 * Refuses an update phones on the channel would never take: one older than an update the channel
 * still offers, or made before the channel last rolled phones back to the code in their build.
 */
async function checkChannelTakes(
  trx: Transaction<Tables>,
  update: StoredUpdate,
  channel: string,
) {
  const newer = await trx
    .selectFrom("updates")
    .innerJoin("update_channels", "update_channels.update_id", "updates.id")
    .select("updates.id")
    .where("update_channels.channel", "=", channel)
    .where("update_channels.withdrawn_at", "is", null)
    .where("updates.platform", "=", update.platform)
    .where("updates.runtime_version", "=", update.runtime_version)
    .where("updates.created_at", ">", update.created_at)
    .executeTakeFirst();
  if (newer)
    refuse(
      `${channel} already offers ${newer.id}, which is newer than ${update.id}; phones would never take it. Withdraw ${newer.id} first.`,
    );
  const rollback = await trx
    .selectFrom("update_rollbacks")
    .select("commit_time")
    .where("channel", "=", channel)
    .where("platform", "=", update.platform)
    .where("runtime_version", "=", update.runtime_version)
    .where("commit_time", ">=", update.created_at)
    .executeTakeFirst();
  if (rollback)
    refuse(
      `${channel} rolled phones back at ${rollback.commit_time.toISOString()}, after ${update.id} was made; phones would never take it. Publish a new update instead.`,
    );
}

/**
 * Offers an already published update on stable too, with the same signed manifest. Refuses a
 * withdrawn one, and one stable's phones would never take.
 */
export async function promoteUpdate(
  db: Kysely<Tables>,
  id: string,
): Promise<Promoted> {
  return db.transaction().execute(async (trx) => {
    const update = await lockUpdate(trx, id);
    const result = {
      id,
      platform: update.platform,
      runtimeVersion: update.runtime_version,
    };

    const channels = await trx
      .selectFrom("update_channels")
      .select(["channel", "withdrawn_at"])
      .where("update_id", "=", id)
      .execute();
    if (channels.some((row) => row.withdrawn_at))
      refuse(`${id} was withdrawn, so it is not offered again.`);
    if (channels.some((row) => row.channel === "stable"))
      return { ...result, added: false };

    await checkChannelTakes(trx, update, "stable");

    await trx
      .insertInto("update_channels")
      .values({ update_id: id, channel: "stable" })
      .execute();
    await audit(trx, {
      userId: null,
      actor: "deploy",
      action: "update.promoted",
      subject: id,
      detail: {
        platform: update.platform,
        runtimeVersion: update.runtime_version,
        from: channels.map((row) => row.channel),
        to: "stable",
      },
    });
    return { ...result, added: true };
  });
}

async function lockUpdate(
  trx: Transaction<Tables>,
  id: string,
): Promise<StoredUpdate> {
  if (!UPDATE_ID.test(id)) refuse(`${id} is not an update id.`);
  const update = await trx
    .selectFrom("updates")
    .select(["id", "platform", "runtime_version", "created_at"])
    .where("id", "=", id)
    .forUpdate()
    .executeTakeFirst();
  return update ?? refuse(`No update has the id ${id}.`);
}

async function withdrawFromChannels(trx: Transaction<Tables>, id: string) {
  const rows = await trx
    .updateTable("update_channels")
    .set({ withdrawn_at: new Date() })
    .where("update_id", "=", id)
    .where("withdrawn_at", "is", null)
    .returning("channel")
    .execute();
  return rows.map((row) => row.channel);
}

export interface Withdrawn {
  id: string;
  platform: string;
  runtimeVersion: string;
  channels: string[];
}

/**
 * Stops offering an update on every channel it is on, so phones that have not taken it never do.
 * Phones already running it keep it; rolling back is what moves them.
 */
export async function withdrawUpdate(
  db: Kysely<Tables>,
  id: string,
): Promise<Withdrawn> {
  return db.transaction().execute(async (trx) => {
    const update = await lockUpdate(trx, id);
    const channels = await withdrawFromChannels(trx, id);
    const result = {
      id,
      platform: update.platform,
      runtimeVersion: update.runtime_version,
      channels,
    };
    if (channels.length === 0) return result;
    await audit(trx, {
      userId: null,
      actor: "deploy",
      action: "update.withdrawn",
      subject: id,
      detail: {
        platform: update.platform,
        runtimeVersion: update.runtime_version,
        channels,
      },
    });
    return result;
  });
}

/** Reads a rollBackToEmbedded directive, refusing anything but exactly the shape phones read. */
function readDirective(directive: Buffer): Date {
  const { parameters } = readJson(directive, "The directive");
  const commitTime = isObject(parameters) ? parameters.commitTime : undefined;
  if (
    typeof commitTime !== "string" ||
    !COMMIT_TIME.test(commitTime) ||
    Number.isNaN(Date.parse(commitTime))
  )
    refuse(
      "The directive's commitTime is not an ISO 8601 UTC time with milliseconds.",
    );
  const expected = JSON.stringify({
    type: "rollBackToEmbedded",
    parameters: { commitTime },
  });
  if (directive.toString("utf8") !== expected)
    refuse(`The directive is not exactly ${expected}.`);
  return new Date(commitTime);
}

export interface RolledBack {
  id: string;
  platform: string;
  runtimeVersion: string;
  channels: string[];
  commitTime: string;
}

/**
 * Withdraws an update and tells every phone on its channels to go back to the code in its build.
 * Phones only obey a directive signed by the update key and made after the update they run.
 */
export async function rollBackUpdate(
  db: Kysely<Tables>,
  {
    id,
    directive,
    signature,
    certificate,
    now = new Date(),
  }: {
    id: string;
    directive: Buffer;
    signature: string;
    certificate: string;
    now?: Date;
  },
): Promise<RolledBack> {
  checkSignature(directive, signature, certificate, [
    "the directive",
    "The directive's signature",
  ]);
  const commitTime = readDirective(directive);
  if (commitTime.getTime() > now.getTime() + CLOCK_SKEW_MS)
    refuse(
      `The directive's commitTime ${commitTime.toISOString()} is in the future.`,
    );

  return db.transaction().execute(async (trx) => {
    const update = await lockUpdate(trx, id);
    if (commitTime <= update.created_at)
      refuse(
        `The directive's commitTime ${commitTime.toISOString()} is not after ${id} was made, so phones running it would ignore it.`,
      );
    const assigned = await trx
      .selectFrom("update_channels")
      .select("channel")
      .where("update_id", "=", id)
      .execute();
    for (const { channel } of assigned) {
      const later = await trx
        .selectFrom("updates")
        .innerJoin("update_channels", "update_channels.update_id", "updates.id")
        .select("updates.id")
        .where("update_channels.channel", "=", channel)
        .where("update_channels.withdrawn_at", "is", null)
        .where("update_channels.assigned_at", ">", (eb) =>
          eb
            .selectFrom("update_channels as rolled_back")
            .select("rolled_back.assigned_at")
            .where("rolled_back.update_id", "=", id)
            .where("rolled_back.channel", "=", channel),
        )
        .where("updates.platform", "=", update.platform)
        .where("updates.runtime_version", "=", update.runtime_version)
        .executeTakeFirst();
      if (later)
        refuse(
          `${channel} has offered ${later.id} since ${id}; rolling back would take phones off it too. Withdraw or roll back ${later.id} instead.`,
        );
    }

    await withdrawFromChannels(trx, id);
    const channels = assigned.map((row) => row.channel);
    await trx
      .insertInto("update_rollbacks")
      .values(
        channels.map((channel) => ({
          update_id: id,
          platform: update.platform,
          runtime_version: update.runtime_version,
          channel,
          directive,
          signature,
          commit_time: commitTime,
        })),
      )
      .execute();
    await audit(trx, {
      userId: null,
      actor: "deploy",
      action: "update.rolled_back",
      subject: id,
      detail: {
        platform: update.platform,
        runtimeVersion: update.runtime_version,
        channels,
        commitTime: commitTime.toISOString(),
      },
    });
    return {
      id,
      platform: update.platform,
      runtimeVersion: update.runtime_version,
      channels,
      commitTime: commitTime.toISOString(),
    };
  });
}
