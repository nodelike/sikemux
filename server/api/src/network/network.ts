import type {
  ChannelVersions,
  MinimumVersions,
  Network,
} from "@sikemux/protocol";

import { isPort, settingsFrom } from "../env.ts";
import { CHANNELS } from "../updates/update.ts";

const APP_VERSION =
  /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$/;

/** Every app version is allowed until a MINIMUM_VERSION_* setting raises it. */
const ANY_VERSION = "0.0.0";

/**
 * Reads the relay and the minimum app versions from the environment:
 *   RELAY_URL, RELAY_REGION, RELAY_QUIC_PORT ("none" when the relay has no QUIC port),
 *   MINIMUM_VERSION_<MACOS|IOS|ANDROID>_<NIGHTLY|STABLE>.
 */
export function readNetwork(
  env: NodeJS.ProcessEnv,
  problems: string[],
): Network {
  const { text: setting } = settingsFrom(env, problems);

  const url = setting("RELAY_URL", "https://relay.sikemux.com/");
  const parsed = URL.parse(url);
  if (parsed?.protocol !== "https:" || parsed.pathname !== "/" || parsed.search)
    problems.push(
      "RELAY_URL is not an https address like https://relay.sikemux.com/",
    );

  const quic = setting("RELAY_QUIC_PORT", "7842");
  const quicPort = quic === "none" ? null : Number(quic);
  if (quicPort !== null && !isPort(quicPort))
    problems.push('RELAY_QUIC_PORT is not a port number or "none"');

  const versions = (platform: keyof MinimumVersions) => {
    const channels = {} as ChannelVersions;
    for (const channel of CHANNELS) {
      const name = `MINIMUM_VERSION_${platform.toUpperCase()}_${channel.toUpperCase()}`;
      const version = setting(name, ANY_VERSION);
      if (!APP_VERSION.test(version))
        problems.push(`${name} is not a version like 0.5.0 or 0.6.0-nightly.3`);
      channels[channel] = version;
    }
    return channels;
  };

  return {
    relays: [
      {
        url: parsed?.href ?? url,
        region: setting("RELAY_REGION", "mumbai"),
        quicPort,
      },
    ],
    minimumVersions: {
      macos: versions("macos"),
      linux: versions("linux"),
      ios: versions("ios"),
      android: versions("android"),
    },
  };
}
