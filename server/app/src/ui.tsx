import { IconCircleCheck } from "@shared/ui/Icons";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ButtonHTMLAttributes,
  type CSSProperties,
  type InputHTMLAttributes,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

import type { Picture, Tone } from "./dots.ts";
import { Mark, useHot } from "./Field.tsx";
import type { SceneName } from "./scenes.ts";

/** A signed-in page: its title over the dots, what can be done on it, and its cards. */
export function Page({
  title,
  tag,
  lede,
  actions,
  mark,
  scene,
  tone,
  children,
}: {
  title: ReactNode;
  tag?: ReactNode;
  lede?: ReactNode;
  actions?: ReactNode;
  mark: Picture;
  scene: SceneName;
  tone?: Tone;
  children?: ReactNode;
}) {
  return (
    <div className="page">
      <header className="head">
        <div className="tx">
          <h1 data-quiet="">
            {title}
            {tag}
          </h1>
          {lede ? (
            <p className="ld" data-quiet="">
              {lede}
            </p>
          ) : null}
          {actions ? <div className="do">{actions}</div> : null}
        </div>
        <Mark picture={mark} scene={scene} tone={tone} />
      </header>
      <div className="bento anim">{children}</div>
    </div>
  );
}

/** A page that stands alone, such as sign-in: words on the left, a large mark on the right. */
export function Hero({
  title,
  lede,
  mark,
  scene,
  tone,
  back,
  foot,
  children,
}: {
  title: ReactNode;
  lede?: ReactNode;
  mark: Picture;
  scene: SceneName;
  tone?: Tone;
  back?: ReactNode;
  foot?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="hero">
      {back ? <div className="back">{back}</div> : null}
      <main className="in">
        <div className="tx">
          <h1 data-quiet="">{title}</h1>
          {lede ? (
            <p className="ld" data-quiet="">
              {lede}
            </p>
          ) : null}
          {children}
        </div>
        <Mark picture={mark} scene={scene} tone={tone} hero />
      </main>
      {foot ? <footer className="foot">{foot}</footer> : null}
    </div>
  );
}

const span = (width: number | undefined) =>
  width ? ({ "--w": width } as CSSProperties) : undefined;

/** A labelled group of things. `w` is its share of the page's twelve columns. */
export function Card({
  label,
  count,
  aside,
  foot,
  w,
  busy,
  children,
}: {
  label?: ReactNode;
  count?: number | string;
  aside?: ReactNode;
  foot?: ReactNode;
  w?: number;
  busy?: boolean;
  children?: ReactNode;
}) {
  return (
    <section
      className={w ? `card w${w}` : "card"}
      style={span(w)}
      data-calm=""
      aria-busy={busy || undefined}
    >
      {label ? (
        <div className="lab-line">
          <h2 className="lab">
            {label}
            {count !== undefined ? <i>{count}</i> : null}
          </h2>
          {aside}
        </div>
      ) : null}
      {busy ? <Skeleton /> : children}
      {foot ? <div className="foot">{foot}</div> : null}
    </section>
  );
}

export function Skeleton() {
  return (
    <div className="skels">
      <div className="skel" />
      <div className="skel" />
      <div className="skel" />
    </div>
  );
}

/** Cards stacked in one column of the page. */
export function Column({
  w,
  side,
  children,
}: {
  w: number;
  side?: boolean;
  children: ReactNode;
}) {
  return (
    <div className={side ? "col side" : `col w${w}`} style={span(w)}>
      {children}
    </div>
  );
}

export function Rows({ children }: { children?: ReactNode }) {
  return <ul className="rows">{children}</ul>;
}

/**
 * A thing in a card: its picture, its name and a line about it, what can be done, and anything open
 * under it. `hot` is what the dots draw while the pointer is over it.
 */
export function Row({
  lead,
  name,
  detail,
  end,
  wrap,
  hot,
  children,
}: {
  lead?: ReactNode;
  name: ReactNode;
  detail?: ReactNode;
  end?: ReactNode;
  wrap?: boolean;
  hot?: { picture: Picture; scene?: SceneName };
  children?: ReactNode;
}) {
  const over = useHot(hot?.picture, hot?.scene);
  return (
    <li className={wrap ? "row wrap" : "row"} {...over}>
      {lead ? <span className="lead">{lead}</span> : <span />}
      <span className="what">
        <span className="nm">{name}</span>
        {detail ? <span className="dt">{detail}</span> : null}
      </span>
      {end ? <span className="end">{end}</span> : null}
      {children ? <div className="more">{children}</div> : null}
    </li>
  );
}

/** A row's icon in a soft square. */
export function Tile({ children }: { children: ReactNode }) {
  return <span className="tile">{children}</span>;
}

export function Btn({
  quiet,
  bad,
  small,
  busy,
  className,
  disabled,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  quiet?: boolean;
  bad?: boolean;
  small?: boolean;
  busy?: boolean;
}) {
  return (
    <button
      type="button"
      className={btnClass({ quiet, bad, small, className })}
      data-busy={busy || undefined}
      disabled={disabled || busy}
      {...props}
    />
  );
}

/** For a link that looks like a button. */
export function btnClass({
  quiet,
  bad,
  small,
  className,
}: {
  quiet?: boolean;
  bad?: boolean;
  small?: boolean;
  className?: string;
}): string {
  return ["btn", quiet && "q", bad && "bad", small && "s", className]
    .filter(Boolean)
    .join(" ");
}

/** A button that reads as text. */
export function Txt({
  bad,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { bad?: boolean }) {
  return (
    <button type="button" className={bad ? "txt bad" : "txt"} {...props} />
  );
}

export function Chip({
  tone,
  children,
}: {
  tone?: "ok" | "bad" | "warn" | "you";
  children: ReactNode;
}) {
  return <span className={tone ? `chip ${tone}` : "chip"}>{children}</span>;
}

export function Field(props: InputHTMLAttributes<HTMLInputElement>) {
  return <input className="field" {...props} />;
}

/** The server's words for what went wrong. */
export function Problem({ children }: { children?: ReactNode }) {
  return children ? (
    <p className="why" role="alert">
      {children}
    </p>
  ) : null;
}

export function Empty({ children }: { children: ReactNode }) {
  return <p className="empty">{children}</p>;
}

/** A menu hung from a button, closed by Escape, a click elsewhere or a choice. */
export function Popover({
  label,
  trigger,
  options,
  disabled,
}: {
  label: string;
  trigger: ReactNode;
  options: {
    key: string;
    name: ReactNode;
    detail?: ReactNode;
    chosen?: boolean;
    bad?: boolean;
    choose: () => void;
  }[];
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (!open) return;
    const away = (event: PointerEvent) => {
      if (!box.current?.contains(event.target as Node)) setOpen(false);
    };
    addEventListener("pointerdown", away);
    return () => removeEventListener("pointerdown", away);
  }, [open]);
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key !== "Escape" || !open) return;
    event.stopPropagation();
    setOpen(false);
    box.current?.querySelector("button")?.focus();
  };
  return (
    <span className="rel" ref={box} onKeyDown={onKeyDown}>
      <button
        type="button"
        className="rolebtn"
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen(!open)}
      >
        {trigger}
      </button>
      {open ? (
        <span className="pop" role="menu">
          {options.map((option) => (
            <button
              key={option.key}
              type="button"
              className={option.bad ? "bad" : undefined}
              role="menuitemradio"
              aria-checked={option.chosen ?? false}
              onClick={() => {
                setOpen(false);
                option.choose();
              }}
            >
              <b>{option.name}</b>
              {option.chosen ? <IconCircleCheck size={16} /> : null}
              {option.detail ? <small>{option.detail}</small> : null}
            </button>
          ))}
        </span>
      ) : null}
    </span>
  );
}

/** Asks before an action inside the row it acts on, and stays open with the problem if it fails. */
export function Confirm({
  id,
  children,
  problem,
  busy,
  label,
  danger,
  unchanged,
  onCancel,
  onConfirm,
}: {
  id?: string;
  children: ReactNode;
  problem?: string;
  busy: boolean;
  label: string;
  danger?: boolean;
  unchanged?: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Escape" && !busy) onCancel();
  };
  return (
    <div className="box" id={id} onKeyDown={onKeyDown}>
      {children}
      <Problem>{problem}</Problem>
      <div className="do">
        <Btn
          small
          bad={danger}
          busy={busy}
          disabled={unchanged}
          onClick={onConfirm}
        >
          {label}
        </Btn>
        <Btn small quiet onClick={onCancel} disabled={busy} autoFocus>
          Cancel
        </Btn>
      </div>
    </div>
  );
}

/** Runs an action that may fail, holding its busy state and the problem to show. */
export function useAction() {
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string>();
  const run = useCallback(
    async (
      action: () => Promise<void>,
      describe: (error: unknown) => string,
    ) => {
      setBusy(true);
      setProblem(undefined);
      try {
        await action();
      } catch (error) {
        setProblem(describe(error));
      } finally {
        setBusy(false);
      }
    },
    [],
  );
  return { busy, problem, run, clear: () => setProblem(undefined) };
}

export function Switch({
  label,
  on,
  disabled,
  onChange,
}: {
  label: string;
  on: boolean;
  disabled?: boolean;
  onChange: (on: boolean) => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      className="sw"
      aria-label={label}
      aria-checked={on}
      disabled={disabled}
      onClick={() => onChange(!on)}
    />
  );
}

export function Seg<Value extends string>({
  label,
  options,
  value,
  disabled,
  onChange,
}: {
  label: string;
  options: { value: Value; name: ReactNode }[];
  value: Value;
  disabled?: boolean;
  onChange: (value: Value) => void;
}) {
  return (
    <span className="seg" role="radiogroup" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          role="radio"
          disabled={disabled}
          aria-checked={option.value === value}
          onClick={() => onChange(option.value)}
        >
          {option.name}
        </button>
      ))}
    </span>
  );
}

type Note = { id: number; text: string; undo?: () => Promise<void> };
type Say = (text: string, undo?: () => Promise<void>) => void;

const ToastContext = createContext<Say>(() => undefined);

/** Says, at the foot of the window, what just changed, with Undo when it can be taken back. */
export function useToast(): Say {
  return useContext(ToastContext);
}

export function Toasts({ children }: { children: ReactNode }) {
  const [note, setNote] = useState<Note>();
  const next = useRef(0);
  useEffect(() => {
    if (!note) return;
    const timer = setTimeout(() => setNote(undefined), 6000);
    return () => clearTimeout(timer);
  }, [note]);
  const say = useCallback<Say>(
    (text, undo) => setNote({ id: ++next.current, text, undo }),
    [],
  );
  return (
    <ToastContext value={say}>
      {children}
      {note
        ? createPortal(
            <Toast key={note.id} note={note} done={() => setNote(undefined)} />,
            document.body,
          )
        : null}
    </ToastContext>
  );
}

function Toast({ note, done }: { note: Note; done: () => void }) {
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const undo = async () => {
    if (!note.undo) return;
    setBusy(true);
    try {
      await note.undo();
      done();
    } catch {
      setFailed(true);
      setBusy(false);
    }
  };
  return (
    <div className="toast" role="status">
      <IconCircleCheck size={17} />
      <span>{failed ? "Undo didn't go through. Try again." : note.text}</span>
      {note.undo ? (
        <Txt onClick={() => void undo()} disabled={busy}>
          Undo
        </Txt>
      ) : null}
    </div>
  );
}
