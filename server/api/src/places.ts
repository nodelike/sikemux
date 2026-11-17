import { readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { Reader, type AsnResponse, type CountryResponse } from "mmdb-lib";

import type { Logger } from "./log.ts";

/** Roughly where an address is: its country and the network that owns it, or null when unknown. */
export interface Places {
  near(address: string): string | null;
}

export const NO_PLACES: Places = { near: () => null };

/** DB-IP's free databases (CC BY 4.0), which setup-citadel.sh refreshes each month. */
const PLACE_FILES = {
  country: "dbip-country-lite.mmdb",
  network: "dbip-asn-lite.mmdb",
} as const;

const RECHECK_MS = 60 * 60_000;

/** Reads the databases in `dir`, and again within an hour of either being replaced. */
export function openPlaces(dir: string, log: Logger): Places {
  let country: Reader<CountryResponse> | null = null;
  let network: Reader<AsnResponse> | null = null;
  let loaded = "";
  const load = () => {
    try {
      const stamp = Object.values(PLACE_FILES)
        .map((file) => statSync(join(dir, file)).mtimeMs)
        .join();
      if (stamp === loaded) return;
      country = new Reader(readFileSync(join(dir, PLACE_FILES.country)));
      network = new Reader(readFileSync(join(dir, PLACE_FILES.network)));
      loaded = stamp;
      log.info({ dir }, "read the IP location databases");
    } catch (error) {
      log.warn({ err: error, dir }, "could not read the IP location databases");
    }
  };
  load();
  setInterval(load, RECHECK_MS).unref();
  return {
    near(address) {
      try {
        const parts = [
          country?.get(address)?.country?.names?.en,
          network?.get(address)?.autonomous_system_organization,
        ].filter((part): part is string => Boolean(part));
        return parts.length ? parts.join(" · ").slice(0, 256) : null;
      } catch {
        return null;
      }
    },
  };
}
