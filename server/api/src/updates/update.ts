import type { ChannelVersions } from "@sikemux/protocol";
import release from "@sikemux/protocol/release.json" with { type: "json" };

/** What phones running Expo's update client ask for, and what a published update must say about itself. */

export const PLATFORMS = release.platforms as readonly ("android" | "ios")[];
export const CHANNELS = release.channels as readonly (keyof ChannelVersions)[];

export type UpdatePlatform = (typeof PLATFORMS)[number];
export type UpdateChannel = (typeof CHANNELS)[number];

export const RUNTIME_VERSION = /^[A-Za-z0-9._+-]{1,256}$/;
export const UPDATE_ID = new RegExp(release.updateId);
export const ASSET_URL = release.assetUrl;

export function isPlatform(value: unknown): value is UpdatePlatform {
  return PLATFORMS.includes(value as UpdatePlatform);
}

export function isChannel(value: unknown): value is UpdateChannel {
  return CHANNELS.includes(value as UpdateChannel);
}
