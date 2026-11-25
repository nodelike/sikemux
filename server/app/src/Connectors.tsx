import { formatAgo } from "@shared/lib/format";
import { useEffect, useId, useRef, useState, type CSSProperties } from "react";

import { drawTile, type Picture } from "./dots.ts";
import { iconMark } from "./marks.ts";
import { pluginSvg } from "./PluginMark.tsx";
import { sceneOf, type SceneName } from "./scenes.ts";
import {
  bySignIn,
  hostOf,
  listed,
  serviceName,
  times,
  type Load,
  type SignIn,
} from "./SignIns.tsx";
import { Btn, btnClass, Card, Empty, Page, Problem, Txt } from "./ui.tsx";

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

const sceneFor = (plugin: string): SceneName =>
  `brand-${plugin.replace(/^sikemux\./, "")}`;

function pictureOf(plugin: string): Picture {
  const svg = pluginSvg(plugin);
  return svg
    ? { svg, color: sceneOf(sceneFor(plugin)).tint }
    : { ...iconMark("key"), color: sceneOf(sceneFor(plugin)).tint };
}

export function ConnectorsPage({ signIns: load }: { signIns: Load }) {
  if (load.state === "failed")
    return (
      <Page
        title="Connectors didn't load"
        lede="Your agents can still use them."
        mark={iconMark("plug")}
        scene="you"
        tone="failed"
        actions={<Btn onClick={() => location.reload()}>Try again</Btn>}
      >
        <div className="full-err">
          <Problem>{load.message}</Problem>
        </div>
      </Page>
    );
  if (load.state === "loading")
    return (
      <Page title="Connectors" mark={iconMark("plug")} scene="you">
        <Card label="Last 30 days" w={12} busy />
      </Page>
    );

  const signIns = bySignIn(load.uses);
  const first = signIns[0];
  if (!first)
    return (
      <Page
        title="Nothing used yet"
        lede="When an agent uses a sign-in from your vault, it shows here."
        mark={iconMark("plug")}
        scene="you"
        tone="waiting"
        actions={
          <a className={btnClass({})} href="sikemux://">
            Open Sikemux
          </a>
        }
      >
        <Card label="Last 30 days" w={12}>
          <Empty>
            No sign-in from your account's vault has been used in the last 30
            days.
          </Empty>
        </Card>
      </Page>
    );

  return (
    <Page
      title="Connectors"
      lede={`The ${COUNTED[signIns.length] ?? `${signIns.length} sign-ins`} your agents used in the last 30 days.`}
      mark={pictureOf(first.plugin)}
      scene={sceneFor(first.plugin)}
    >
      <div className="tiles">
        {signIns.map((signIn) => (
          <ConnectorTile key={signIn.entryId} signIn={signIn} />
        ))}
      </div>
    </Page>
  );
}

const TILE_SIZE = 120;

function ConnectorTile({ signIn }: { signIn: SignIn }) {
  const [open, setOpen] = useState(false);
  const breakdownId = useId();
  const picture = pictureOf(signIn.plugin);
  const scene = sceneFor(signIn.plugin);
  const card = useRef<HTMLElement>(null);
  const agents = new Set(
    signIn.uses.map((use) => use.actor).filter((actor) => actor !== "Sikemux"),
  );
  const [only] = agents;
  const by = [
    ...(agents.size === 1 && only ? [only] : []),
    ...(agents.size > 1 ? [`${agents.size} agents`] : []),
    ...(signIn.uses.some((use) => use.actor === "Sikemux") ? ["Sikemux"] : []),
  ];
  const hosts = [...new Set(signIn.uses.map(hostOf))];

  return (
    <article
      ref={card}
      className="ct"
      style={{ "--bt": sceneOf(scene).tint } as CSSProperties}
    >
      <TileMark picture={picture} card={card} />
      <b>{serviceName(signIn.plugin)}</b>
      <span className="dt">
        {listed(by)} on{" "}
        {hosts.length > 2 ? `${hosts.length} hosts` : listed(hosts)}
      </span>
      <span className="u">
        <span>
          {signIn.count === 1
            ? "1 use"
            : `${signIn.count.toLocaleString()} uses`}
        </span>
        <span>{formatAgo(signIn.lastAt)}</span>
      </span>
      <Txt
        aria-label={`Details of ${serviceName(signIn.plugin)}`}
        aria-expanded={open}
        aria-controls={open ? breakdownId : undefined}
        onClick={() => setOpen(!open)}
      >
        {open ? "Less" : "Details"}
      </Txt>
      {open ? (
        <ul id={breakdownId} className="box">
          {signIn.uses.map((use) => (
            <li key={`${use.hostKey}-${use.actor}`}>
              {use.actor} on {hostOf(use)}
              <br />
              {times(use.count)} · {formatAgo(use.lastAt)}
            </li>
          ))}
        </ul>
      ) : null}
    </article>
  );
}

/** The brand in dots, or its plain picture where dots can't be drawn. */
function TileMark({
  picture,
  card,
}: {
  picture: Picture;
  card: { readonly current: HTMLElement | null };
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [plain, setPlain] = useState(false);
  const key = JSON.stringify(picture);
  useEffect(() => {
    if (!canvas.current || !card.current) return;
    const stop = drawTile(canvas.current, JSON.parse(key) as Picture, {
      size: TILE_SIZE,
      hot: card.current,
    });
    setPlain(!stop);
    return stop ?? undefined;
  }, [key, card]);
  return (
    <span className="ct-mark" aria-hidden="true">
      <canvas ref={canvas} hidden={plain} />
      {plain && "svg" in picture ? (
        <span
          className="ct-plain"
          dangerouslySetInnerHTML={{ __html: picture.svg }}
        />
      ) : null}
    </span>
  );
}
