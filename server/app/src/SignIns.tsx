import { useAuth } from "@clerk/react";
import type { AccountEvent, VaultUseRow } from "@sikemux/protocol";
import { useCallback, useEffect, useRef, useState } from "react";

import { api, problemText, signedInToken } from "./api.ts";

const SERVICE_NAMES: Record<string, string> = {
  "sikemux.github": "GitHub",
  "sikemux.gitlab": "GitLab",
  "sikemux.bitbucket": "Bitbucket",
  "sikemux.cloudflare": "Cloudflare",
  "sikemux.forgejo": "Forgejo",
  "sikemux.jira": "Jira",
  "sikemux.linear": "Linear",
  "sikemux.slack": "Slack",
  "sikemux.vercel": "Vercel",
  "sikemux.signoz": "SigNoz",
  "sikemux.rundeck": "Rundeck",
  "sikemux.googleplay": "Google Play",
  "sikemux.database": "Database",
};

export const serviceName = (plugin: string) => SERVICE_NAMES[plugin] ?? plugin;

export type Load =
  | { state: "loading" }
  | { state: "failed"; message: string }
  | { state: "loaded"; uses: VaultUseRow[] };

/** Who used the vault's sign-ins lately, read once and again whenever the vault changes. */
export function useSignIns(ready: boolean) {
  const { isSignedIn, getToken } = useAuth();
  const [load, setLoad] = useState<Load>({ state: "loading" });
  const reading = useRef(0);

  const read = useCallback(async () => {
    const attempt = ++reading.current;
    try {
      const token = await signedInToken(getToken, "see your sign-ins");
      const { uses } = await api(token).listVaultUses();
      if (attempt === reading.current) setLoad({ state: "loaded", uses });
    } catch (error) {
      if (attempt !== reading.current) return;
      setLoad((current) =>
        current.state === "loaded"
          ? current
          : { state: "failed", message: problemText(error) },
      );
    }
  }, [getToken]);

  useEffect(() => {
    if (!ready || !isSignedIn) return;
    void read();
    return () => {
      reading.current += 1;
    };
  }, [ready, isSignedIn, read]);

  const apply = (events: AccountEvent[]) => {
    if (events.some((event) => event.type === "vault.changed")) void read();
  };

  return { load, reload: () => void read(), apply };
}

export interface SignIn {
  entryId: string;
  plugin: string;
  count: number;
  lastAt: string;
  uses: VaultUseRow[];
}

/** The rows come one for each entry, host and agent; a sign-in is one entry. */
export function bySignIn(uses: VaultUseRow[]): SignIn[] {
  const signIns = new Map<string, SignIn>();
  for (const use of uses) {
    const signIn = signIns.get(use.entryId);
    if (signIn === undefined) {
      signIns.set(use.entryId, {
        entryId: use.entryId,
        plugin: use.plugin,
        count: use.count,
        lastAt: use.lastAt,
        uses: [use],
      });
      continue;
    }
    signIn.count += use.count;
    if (use.lastAt > signIn.lastAt) signIn.lastAt = use.lastAt;
    signIn.uses.push(use);
  }
  return [...signIns.values()].sort((a, b) => b.lastAt.localeCompare(a.lastAt));
}

export const hostOf = (use: VaultUseRow) => use.hostName ?? "a host that left";
