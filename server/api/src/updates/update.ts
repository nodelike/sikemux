import type { ChannelVersions } from "@sikemux/protocol";

/** What phones running Expo's update client ask for, and what a published update must say about itself. */

export const PLATFORMS = ["android", "ios"] as const;
export const CHANNELS = [
  "nightly",
  "stable",
] as const satisfies readonly (keyof ChannelVersions)[];

export type UpdatePlatform = (typeof PLATFORMS)[number];
export type UpdateChannel = (typeof CHANNELS)[number];

export const RUNTIME_VERSION = /^[A-Za-z0-9._+-]{1,256}$/;
export const UPDATE_ID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
export const ASSET_URL = "https://updates.sikemux.com/assets/";

export function isPlatform(value: unknown): value is UpdatePlatform {
  return PLATFORMS.includes(value as UpdatePlatform);
}

export function isChannel(value: unknown): value is UpdateChannel {
  return CHANNELS.includes(value as UpdateChannel);
}
