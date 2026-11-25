import { formatAgo } from "@shared/lib/format";
import {
  IconChevron,
  IconExternal,
  IconKey,
  IconRefresh,
} from "@shared/ui/Icons";
import { useId, useState } from "react";

import { iconMark } from "./marks.ts";
import { PluginMark } from "./PluginMark.tsx";
import {
  bySignIn,
  hostOf,
  serviceName,
  type Load,
  type SignIn,
} from "./SignIns.tsx";
import { Btn, btnClass, Nothing, Page, Problem, Section } from "./ui.tsx";
import "./hosts.css";

const COUNTED = [
  "",
  "one sign-in",
  "two sign-ins",
  "three sign-ins",
  "four sign-ins",
  "five sign-ins",
  "six sign-ins",
  "seven sign-ins",
  "eight sign-ins",
  "nine sign-ins",
];

function formatActor(actor: string): string {
  if (actor === "Sikemux" || actor.startsWith("@")) return actor;
  return `@${actor}`;
}

export function ConnectorsPage({ signIns: load }: { signIns: Load }) {
  const [openId, setOpenId] = useState<string | null>(null);

  if (load.state === "failed")
    return (
      <Page
        title="Connectors"
        lede="The sign-ins your agents used in the last 30 days."
        mark={iconMark("plug")}
        scene="you"
        tone="failed"
      >
        <Nothing
          title="Connectors didn't load"
          actions={
            <Btn quiet onClick={() => location.reload()}>
              <IconRefresh size={14} />
              Try again
            </Btn>
          }
        >
          Your agents can still use them.
        </Nothing>
        <Problem>{load.message}</Problem>
      </Page>
    );

  if (load.state === "loading")
    return (
      <Page
        title="Connectors"
        lede="The sign-ins your agents used in the last 30 days."
        mark={iconMark("plug")}
        scene="you"
      >
        <Section label="Last 30 days" busy />
      </Page>
    );

  const signIns = bySignIn(load.uses);
  if (signIns.length === 0)
    return (
      <Page
        title="Connectors"
        lede="The sign-ins your agents used in the last 30 days."
        mark={iconMark("plug")}
        scene="you"
      >
        <Nothing
          title="Nothing used yet"
          actions={
            <a className={btnClass({})} href="sikemux://">
              <IconExternal size={14} />
              Open Sikemux
            </a>
          }
        >
          When an agent uses a sign-in from your vault, it shows here.
        </Nothing>
      </Page>
    );

  const maxCount = Math.max(...signIns.map((signIn) => signIn.count), 1);

  return (
    <Page
      title="Connectors"
      lede={`The ${COUNTED[signIns.length] ?? `${signIns.length} sign-ins`} your agents used in the last 30 days.`}
      mark={iconMark("plug")}
      scene="you"
    >
      <div className="con-list">
        {signIns.map((signIn) => (
          <ConnectorRow
            key={signIn.entryId}
            signIn={signIn}
            maxCount={maxCount}
            open={openId === signIn.entryId}
            onToggle={() =>
              setOpenId(openId === signIn.entryId ? null : signIn.entryId)
            }
          />
        ))}
      </div>
    </Page>
  );
}

function ConnectorRow({
  signIn,
  maxCount,
  open,
  onToggle,
}: {
  signIn: SignIn;
  maxCount: number;
  open: boolean;
  onToggle: () => void;
}) {
  const breakdownId = useId();
  const agents = [
    ...new Set(
      signIn.uses
        .map((use) => use.actor)
        .filter((actor) => actor !== "Sikemux")
        .map(formatActor),
    ),
  ];
  const by = [
    ...(agents.length > 2 ? [`${agents.length} agents`] : agents),
    ...(signIn.uses.some((use) => use.actor === "Sikemux") ? ["Sikemux"] : []),
  ];
  const hosts = [...new Set(signIn.uses.map(hostOf))];

  return (
    <div className={open ? "open" : undefined}>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={open ? breakdownId : undefined}
        onClick={onToggle}
      >
        <PluginMark
          plugin={signIn.plugin}
          size={30}
          fallback={<IconKey size={20} />}
        />
        <span className="con-name">
          <b>{serviceName(signIn.plugin)}</b>
          <small>
            {by.join(", ")} on{" "}
            {hosts.length > 1 ? `${hosts.length} hosts` : hosts[0]}
          </small>
        </span>
        <span className="con-bar" aria-hidden="true">
          <i
            style={{
              width: `${Math.max(1.5, (signIn.count / maxCount) * 100)}%`,
            }}
          />
        </span>
        <span className="con-uses">
          {signIn.count.toLocaleString()}
          <small>{signIn.count === 1 ? "use" : "uses"}</small>
        </span>
        <span className="con-when">{formatAgo(signIn.lastAt)}</span>
        <IconChevron size={14} className="con-chev" />
      </button>
      {open ? (
        <ul id={breakdownId}>
          {signIn.uses.map((use) => (
            <li key={`${use.hostKey}-${use.actor}`}>
              <span>{formatActor(use.actor)}</span>
              <small>{hostOf(use)}</small>
              <span className="con-bar" aria-hidden="true">
                <i
                  style={{
                    width: `${Math.max(1.5, (use.count / maxCount) * 100)}%`,
                  }}
                />
              </span>
              <em>{use.count.toLocaleString()}</em>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
