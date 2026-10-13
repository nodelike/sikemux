import { pushAppValues, type PushApp } from "@sikemux/protocol";

import { readAppleKeySettings, type AppleKey } from "../apple-key.ts";
import { settingsFrom } from "../env.ts";
import { APNS_TOPICS } from "./apns.ts";
import { readServiceAccount, type ServiceAccount } from "./fcm.ts";

export interface PushSettings {
  /** The one phone app whose tokens this server accepts and whose credentials it holds. */
  app: PushApp;
  /** Whether a production server accepts iOS tokens from Apple's sandbox, for builds run from Xcode. */
  allowSandbox: boolean;
  /** The Firebase service account FCM sends as, or null when Android pushes are not set up. */
  fcm: ServiceAccount | null;
  /** The key APNs takes this server's pushes with, or null when iPhone pushes are not set up. */
  apns: (AppleKey & { topic: string }) | null;
}

/**
 * Reads PUSH_APP ("production" by default), APNS_ALLOW_SANDBOX ("1" to allow),
 * FCM_SERVICE_ACCOUNT_FILE, the path of the Firebase service account's JSON key, and
 * APNS_KEY_FILE, the path of Apple's .p8 push key, with its APNS_KEY_ID and APNS_TEAM_ID.
 */
export function readPush(
  env: NodeJS.ProcessEnv,
  problems: string[],
): PushSettings {
  const settings = settingsFrom(env, problems);
  const app = settings.oneOf("PUSH_APP", pushAppValues, "production");
  const sandbox = settings.flag("APNS_ALLOW_SANDBOX", false);

  const path = settings.optional("FCM_SERVICE_ACCOUNT_FILE");
  let fcm: ServiceAccount | null = null;
  if (path) {
    try {
      fcm = readServiceAccount(path);
    } catch (error) {
      problems.push(
        `FCM_SERVICE_ACCOUNT_FILE ${path} ${(error as Error).message}`,
      );
    }
  }

  return {
    app,
    allowSandbox: app === "dev" || sandbox,
    fcm,
    apns: readApns(env, app, problems),
  };
}

function readApns(
  env: NodeJS.ProcessEnv,
  app: PushApp,
  problems: string[],
): PushSettings["apns"] {
  const key = readAppleKeySettings(env, "APNS", problems);
  return key && { ...key, topic: APNS_TOPICS[app] };
}
