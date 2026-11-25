import type { ReactNode } from "react";

import lockup from "../../../brand/wordmark/lockup-purple.svg";

import type { Picture, Tone } from "./dots.ts";
import { Mark } from "./Field.tsx";
import "./gate.css";
import { brandMark } from "./marks.ts";
import type { SceneName } from "./scenes.ts";

/** One layout for every standalone page outside the rail. */
export function Gate({
  title,
  lede,
  mark = brandMark,
  scene = "sign",
  tone,
  back,
  say,
  foot = true,
  children,
}: {
  title: ReactNode;
  lede?: ReactNode;
  mark?: Picture;
  scene?: SceneName;
  tone?: Tone;
  back?: ReactNode;
  say?: ReactNode;
  foot?: boolean | ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="gt">
      <div className="gtop">{back ?? null}</div>
      <main className="gin">
        <div className="gtx" data-quiet="">
          <h1>{title}</h1>
          {lede ? <p className="gl">{lede}</p> : null}
          <div className="gb">{children}</div>
          {say ? <p className="gs">{say}</p> : null}
        </div>
        <Mark hero picture={mark} scene={scene} tone={tone} />
      </main>
      {foot === false ? (
        <div className="gfoot" />
      ) : (
        <footer className="gfoot">{foot === true ? <Foot /> : foot}</footer>
      )}
    </div>
  );
}

export function GateActions({ children }: { children?: ReactNode }) {
  return <div className="gdo">{children}</div>;
}

export function GateFacts({
  items,
}: {
  items: {
    key: string;
    icon?: ReactNode;
    title: ReactNode;
    detail?: ReactNode;
  }[];
}) {
  return (
    <ul className="gfacts">
      {items.map((item) => (
        <li key={item.key}>
          {item.icon}
          <span>
            <b>{item.title}</b>
            {item.detail ? <small>{item.detail}</small> : null}
          </span>
        </li>
      ))}
    </ul>
  );
}

export function GateWho({
  lead,
  name,
  detail,
  action,
}: {
  lead: ReactNode;
  name: ReactNode;
  detail?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="gwho">
      {lead}
      <span className="gwho-tx">
        <b>{name}</b>
        {detail ? <small>{detail}</small> : null}
      </span>
      {action}
    </div>
  );
}

export function Foot() {
  return (
    <>
      <img src={lockup} alt="Sikemux" />
      <a href="https://sikemux.com/privacy">Privacy</a>
      <a href="https://sikemux.com/terms">Terms</a>
    </>
  );
}

export function Loading({ title = "Loading…" }: { title?: string }) {
  return (
    <Gate
      title={title}
      mark={brandMark}
      scene="sign"
      tone="waiting"
      foot={false}
    />
  );
}
