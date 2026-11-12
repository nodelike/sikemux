import { useAuth } from "@clerk/react";
import type { AccountEvent, VaultUseRow } from "@sikemux/protocol";
import { useCallback, useEffect, useRef, useState } from "react";

import { formatAgo } from "@shared/lib/format";
import { api, problemText, signedInToken } from "./api.ts";
import { IconKey } from "@shared/ui/Icons";

const SERVICE_NAMES: Record<string, string> = {
  "sikemux.github": "GitHub",
  "sikemux.gitlab": "GitLab",
  "sikemux.bitbucket": "Bitbucket",
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

type Load =
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

export function SignIns({ load }: { load: Load }) {
  if (load.state === "failed") return <p className="problem">{load.message}</p>;
  const uses = load.state === "loaded" ? load.uses : null;
  return (
    <div className="groups">
      <section className="group">
        <h2>
          Sign-ins
          {uses ? <span className="count">{uses.length}</span> : null}
        </h2>
        {uses === null ? (
          <ul className="rows" aria-busy="true">
            <li className="row placeholder" />
          </ul>
        ) : uses.length === 0 ? (
          <p className="empty">
            No sign-in from your account's vault has been used in the last 30
            days.
          </p>
        ) : (
          <ul className="rows">
            {uses.map((use) => (
              <SignInUse
                key={`${use.entryId}-${use.hostKey}-${use.actor}-${use.lastAt}`}
                use={use}
              />
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

function SignInUse({ use }: { use: VaultUseRow }) {
  const where = use.hostName ?? "a host that left";
  const times = use.count === 1 ? "once" : `${use.count} times`;
  return (
    <li className="row">
      <span className="glyph">
        <IconKey size={18} />
      </span>
      <span className="name">{SERVICE_NAMES[use.plugin] ?? use.plugin}</span>
      <span className="detail">
        {`used by ${use.actor} on ${where} · ${times} · ${formatAgo(use.lastAt)}`}
      </span>
    </li>
  );
}
