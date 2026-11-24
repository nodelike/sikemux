import { useAuth } from "@clerk/react";
import type { AccountEvent, VaultUseRow } from "@sikemux/protocol";
import { useCallback, useEffect, useId, useRef, useState } from "react";

import { formatAgo } from "@shared/lib/format";
import { api, problemText, signedInToken } from "./api.ts";
import { IconKey } from "@shared/ui/Icons";
import { PluginMark } from "./PluginMark.tsx";
import { ItemRow, Rows, Section } from "./old.tsx";

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

interface SignIn {
  entryId: string;
  plugin: string;
  count: number;
  lastAt: string;
  uses: VaultUseRow[];
}

/** The rows come one for each entry, host and agent; a sign-in is one entry. */
function bySignIn(uses: VaultUseRow[]): SignIn[] {
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

const hostOf = (use: VaultUseRow) => use.hostName ?? "a host that left";

const times = (count: number) =>
  count === 1 ? "once" : `${count.toLocaleString()} times`;

function listed(names: string[]): string {
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

export function SignIns({ load }: { load: Load }) {
  if (load.state === "failed") return <p className="problem">{load.message}</p>;
  const signIns = load.state === "loaded" ? bySignIn(load.uses) : null;
  return (
    <div className="groups">
      <Section label="Sign-ins" count={signIns?.length}>
        {signIns === null ? (
          <Rows busy />
        ) : signIns.length === 0 ? (
          <p className="empty">
            No sign-in from your account's vault has been used in the last 30
            days.
          </p>
        ) : (
          <Rows>
            {signIns.map((signIn) => (
              <SignInRow key={signIn.entryId} signIn={signIn} />
            ))}
          </Rows>
        )}
      </Section>
    </div>
  );
}

function SignInRow({ signIn }: { signIn: SignIn }) {
  const [open, setOpen] = useState(false);
  const breakdownId = useId();
  const actors = new Set(signIn.uses.map((use) => use.actor));
  const agents = [...actors].filter((actor) => actor !== "Sikemux").length;
  const by = [
    ...(actors.has("Sikemux") ? ["Sikemux"] : []),
    ...(agents > 0 ? [agents === 1 ? "1 agent" : `${agents} agents`] : []),
  ];
  const hosts = [...new Set(signIn.uses.map(hostOf))];
  return (
    <ItemRow
      glyph={
        <PluginMark
          plugin={signIn.plugin}
          size={18}
          fallback={<IconKey size={18} />}
        />
      }
      name={SERVICE_NAMES[signIn.plugin] ?? signIn.plugin}
      detail={`used ${times(signIn.count)} by ${listed(by)} on ${listed(hosts)} · ${formatAgo(signIn.lastAt)}`}
      end={
        <button
          type="button"
          className="remove"
          aria-expanded={open}
          aria-controls={open ? breakdownId : undefined}
          onClick={() => setOpen(!open)}
        >
          {open ? "Hide" : "Details"}
        </button>
      }
    >
      {open ? (
        <ul id={breakdownId} className="signin-uses">
          {signIn.uses.map((use) => (
            <li key={`${use.hostKey}-${use.actor}`}>
              <span>
                {use.actor} on {hostOf(use)}
              </span>
              <span className="quiet">
                {times(use.count)} · {formatAgo(use.lastAt)}
              </span>
            </li>
          ))}
        </ul>
      ) : null}
    </ItemRow>
  );
}
