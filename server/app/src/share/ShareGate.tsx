import type { Dots, DotsMark } from "@shared/signin/dots.js";
import { useEffect, useRef } from "react";

import sikemuxLockup from "../../../../brand/wordmark/lockup-purple.svg";
import { GET_SIKEMUX } from "./ReaderBar.tsx";

export type GateIcon =
  "message" | "file" | "link" | "lock" | "laptop" | "clock";

export type GateTone = "waiting" | "amber" | "failed";

export type GateAction =
  { label: string; href: string } | { label: string; onClick: () => void };

const PATHS: Record<GateIcon, string> = {
  message:
    '<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>',
  file: '<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M10 9H8"/><path d="M16 13H8"/><path d="M16 17H8"/>',
  link: '<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>',
  lock: '<rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>',
  laptop:
    '<path d="M18 5a2 2 0 0 1 2 2v8.526a2 2 0 0 0 .212.897l1.068 2.127a1 1 0 0 1-.9 1.45H3.62a1 1 0 0 1-.9-1.45l1.068-2.127A2 2 0 0 0 4 15.526V7a2 2 0 0 1 2-2z"/><path d="M20.054 15.987H3.946"/>',
  clock:
    '<circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/>',
};

const LOOKS: Record<
  GateTone,
  {
    tint: string;
    alt: string;
    bright: string;
    deep: string;
    tone: "worked" | "failed" | "waiting";
  }
> = {
  waiting: {
    tint: "#a277ff",
    alt: "#ff6ac1",
    bright: "#f6f1ff",
    deep: "#4a2ca8",
    tone: "waiting",
  },
  amber: {
    tint: "#ffca85",
    alt: "#ff9e5e",
    bright: "#fff8ec",
    deep: "#8f5a1c",
    tone: "worked",
  },
  failed: {
    tint: "#a277ff",
    alt: "#ff6ac1",
    bright: "#f6f1ff",
    deep: "#4a2ca8",
    tone: "failed",
  },
};

function iconSvg(icon: GateIcon): string {
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${PATHS[icon]}</svg>`;
}

export function homeAction(signedIn: boolean): GateAction {
  return signedIn
    ? { label: "Go to your account", href: "/" }
    : { label: "Go to sikemux.com", href: GET_SIKEMUX };
}

export function ShareGate({
  title,
  lede,
  icon,
  tone,
  action,
  foot = true,
}: {
  title: string;
  lede?: string;
  icon: GateIcon;
  tone: GateTone;
  action?: GateAction;
  foot?: boolean;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const spotRef = useRef<HTMLDivElement>(null);
  const textRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    const spot = spotRef.current;
    if (!canvas || !spot) return;

    const svg = iconSvg(icon);
    spot.innerHTML = svg;
    const mark: DotsMark = { svg, color: "#e7e5ef" };
    const look = LOOKS[tone];

    let cancelled = false;
    let dots: Dots | null = null;
    let observer: ResizeObserver | null = null;

    void import("@shared/signin/dots.js").then(({ drawDots }) => {
      if (cancelled) return;
      dots = drawDots(canvas, {
        mark: spot,
        quiet: () => (textRef.current ? [textRef.current] : []),
      });
      if (!dots) return;
      dots.look({
        ...look,
        glow: 0.16,
        reach: 0.58,
        rich: 0.9,
        clearing: 0.5,
      });
      dots.show(mark);
      dots.measure();
      observer = new ResizeObserver(() => dots?.measure());
      if (spot.parentElement) observer.observe(spot.parentElement);
      const main = spot.closest("main");
      if (main) observer.observe(main);
    });

    return () => {
      cancelled = true;
      observer?.disconnect();
      dots?.stop();
    };
  }, [icon, tone]);

  return (
    <div className={`sh-gate-wrap tone-${tone}`}>
      <canvas className="sh-dots" ref={canvasRef} aria-hidden="true" />
      <div className="sh-gate">
        <div className="sh-gate-top" />
        <main
          className="sh-gate-in"
          role={tone === "failed" ? "alert" : "status"}
        >
          <div className="sh-gate-tx" ref={textRef}>
            <h1>{title}</h1>
            {lede ? <p className="sh-gate-lede">{lede}</p> : null}
            {action ? (
              <div className="sh-gate-do">
                {"href" in action ? (
                  <a className="sh-btn q" href={action.href}>
                    {action.label}
                  </a>
                ) : (
                  <button
                    type="button"
                    className="sh-btn q"
                    onClick={action.onClick}
                  >
                    {action.label}
                  </button>
                )}
              </div>
            ) : null}
          </div>
          <div className="sh-markbox" aria-hidden="true">
            <div className="sh-spot" ref={spotRef} />
          </div>
        </main>
        {foot ? (
          <footer className="sh-gate-foot">
            <a className="sh-brand" href={GET_SIKEMUX} aria-label="Sikemux">
              <img src={sikemuxLockup} alt="" />
            </a>
            <span className="sh-gap" />
            <a href="https://sikemux.com/privacy">Privacy</a>
            <a href="https://sikemux.com/terms">Terms</a>
          </footer>
        ) : (
          <div className="sh-gate-foot" />
        )}
      </div>
    </div>
  );
}
