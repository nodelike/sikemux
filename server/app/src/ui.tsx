import { IconCircleCheck, IconClose, IconSearch } from "@shared/ui/Icons";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type ButtonHTMLAttributes,
  type CSSProperties,
  type HTMLAttributes,
  type InputHTMLAttributes,
  type KeyboardEvent,
  type ReactNode,
  type SyntheticEvent,
} from "react";
import { createPortal } from "react-dom";

import type { Picture, Tone } from "./dots.ts";
import { Mark } from "./Field.tsx";
import type { SceneName } from "./scenes.ts";

/** A signed-in page: its title beside its mark in the dots, what needs saying, its tools, and its parts. */
export function Page({
  title,
  tag,
  lede,
  actions,
  notice,
  tools,
  mark,
  scene,
  tone,
  children,
}: {
  title: ReactNode;
  tag?: ReactNode;
  lede?: ReactNode;
  actions?: ReactNode;
  notice?: ReactNode;
  tools?: ReactNode;
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
      {notice}
      {tools ? (
        <div className="tools" data-quiet="">
          {tools}
        </div>
      ) : null}
      <div className="body anim" data-quiet="">
        {children}
      </div>
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

/** A labelled part of a page, marked off by its heading's line alone. */
export function Section({
  label,
  count,
  aside,
  foot,
  busy,
  className,
  children,
  ...props
}: HTMLAttributes<HTMLElement> & {
  label?: ReactNode;
  count?: number | string;
  aside?: ReactNode;
  foot?: ReactNode;
  busy?: boolean;
  children?: ReactNode;
}) {
  return (
    <section
      {...props}
      className={["sect", className].filter(Boolean).join(" ")}
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

export function Rows({ children }: { children?: ReactNode }) {
  return <ul className="rows">{children}</ul>;
}

/** A thing in a card: its picture, its name and a line about it, what can be done, and anything open under it. */
export function Row({
  lead,
  name,
  detail,
  end,
  wrap,
  children,
}: {
  lead?: ReactNode;
  name: ReactNode;
  detail?: ReactNode;
  end?: ReactNode;
  wrap?: boolean;
  children?: ReactNode;
}) {
  return (
    <li className={wrap ? "row wrap" : "row"}>
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

/** A menu hung from a button, closed by Escape, a click elsewhere or a choice. `pick` draws the button as a field to choose from, `icon` as a square holding an icon alone. */
export function Popover({
  label,
  trigger,
  options,
  disabled,
  look,
  align = "right",
}: {
  label: string;
  trigger: ReactNode;
  options: {
    key: string;
    name: ReactNode;
    icon?: ReactNode;
    detail?: ReactNode;
    chosen?: boolean;
    bad?: boolean;
    choose: () => void;
  }[];
  disabled?: boolean;
  look?: "pick" | "icon";
  align?: "left" | "right";
}) {
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLSpanElement>(null);
  const button = useRef<HTMLButtonElement>(null);
  const items = () => [
    ...(box.current?.querySelectorAll<HTMLElement>("[role^=menuitem]") ?? []),
  ];
  useEffect(() => {
    if (!open) return;
    const all = items();
    (all.find((item) => item.ariaChecked === "true") ?? all[0])?.focus();
    const away = (event: PointerEvent) => {
      if (!box.current?.contains(event.target as Node)) setOpen(false);
    };
    addEventListener("pointerdown", away);
    return () => removeEventListener("pointerdown", away);
  }, [open]);
  const close = () => {
    setOpen(false);
    button.current?.focus();
  };
  const onKeyDown = (event: KeyboardEvent) => {
    if (!open) {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        setOpen(true);
      }
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close();
      return;
    }
    if (event.key === "Tab") {
      setOpen(false);
      return;
    }
    const all = items();
    const at = all.indexOf(document.activeElement as HTMLElement);
    const to = {
      ArrowDown: (at + 1) % all.length,
      ArrowUp: (at - 1 + all.length) % all.length,
      Home: 0,
      End: all.length - 1,
    }[event.key];
    if (to === undefined) return;
    event.preventDefault();
    all[to]?.focus();
  };
  const choices = options.some((option) => option.chosen !== undefined);
  return (
    <span className="rel" ref={box} onKeyDown={onKeyDown}>
      <button
        ref={button}
        type="button"
        className={
          look === "pick" ? "pick" : look === "icon" ? "ico" : "rolebtn"
        }
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen(!open)}
      >
        {trigger}
      </button>
      {open ? (
        <span className={align === "left" ? "pop left" : "pop"} role="menu">
          {options.map((option) => (
            <button
              key={option.key}
              type="button"
              tabIndex={-1}
              className={option.bad ? "bad" : undefined}
              role={choices ? "menuitemradio" : "menuitem"}
              aria-checked={choices ? (option.chosen ?? false) : undefined}
              onClick={() => {
                close();
                option.choose();
              }}
            >
              <b>
                {option.icon}
                {option.name}
              </b>
              {option.chosen ? <IconCircleCheck size={16} /> : null}
              {option.detail ? <small>{option.detail}</small> : null}
            </button>
          ))}
        </span>
      ) : null}
    </span>
  );
}

/** A window over the page. Escape, Close and a click outside call `onClose`, unless it is busy. */
export function Dialog({
  id,
  title,
  wide,
  busy,
  foot,
  onClose,
  onSubmit,
  children,
}: {
  id?: string;
  title: ReactNode;
  wide?: boolean;
  busy?: boolean;
  foot?: ReactNode;
  onClose: () => void;
  onSubmit?: () => void;
  children?: ReactNode;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const heading = useId();
  useLayoutEffect(() => {
    const element = dialog.current;
    if (!element) return;
    element.showModal();
    (
      element.querySelector<HTMLElement>(
        ".db :is(input, textarea, [aria-checked='true'])",
      ) ?? element.querySelector<HTMLElement>("footer button:not(:disabled)")
    )?.focus();
    return () => element.close();
  }, []);
  const leave = () => {
    if (!busy) onClose();
  };
  const onCancel = (event: SyntheticEvent) => {
    event.preventDefault();
    leave();
  };
  return createPortal(
    <dialog
      ref={dialog}
      id={id}
      className={wide ? "dlg wide" : "dlg"}
      aria-labelledby={heading}
      onCancel={onCancel}
      onPointerDown={(event) => {
        if (event.target === event.currentTarget) leave();
      }}
    >
      <form
        onSubmit={(event) => {
          event.preventDefault();
          event.stopPropagation();
          if (!busy) onSubmit?.();
        }}
      >
        <header>
          <h2 id={heading}>{title}</h2>
          <button
            type="button"
            className="ico"
            aria-label="Close"
            disabled={busy}
            onClick={leave}
          >
            <IconClose size={15} />
          </button>
        </header>
        <div className="db">{children}</div>
        {foot ? <footer>{foot}</footer> : null}
      </form>
    </dialog>,
    document.body,
  );
}

/** Asks before an action, and stays open with the problem if it fails. */
export function Confirm({
  id,
  title,
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
  title?: ReactNode;
  children: ReactNode;
  problem?: string;
  busy: boolean;
  label: string;
  danger?: boolean;
  unchanged?: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <Dialog
      id={id}
      title={title ?? label}
      busy={busy}
      onClose={onCancel}
      onSubmit={unchanged ? undefined : onConfirm}
      foot={
        <>
          <Btn quiet onClick={onCancel} disabled={busy} autoFocus={danger}>
            Cancel
          </Btn>
          <Btn type="submit" bad={danger} busy={busy} disabled={unchanged}>
            {label}
          </Btn>
        </>
      }
    >
      {children}
      <Problem>{problem}</Problem>
    </Dialog>
  );
}

/** Rows under column headings. `cols` is their widths; a phone keeps the first cell and the last. */
export function Table({
  label,
  cols,
  heads,
  children,
}: {
  label: string;
  cols: string;
  heads: ReactNode[];
  children?: ReactNode;
}) {
  return (
    <div
      className="tbl"
      role="table"
      aria-label={label}
      style={{ "--cols": cols } as CSSProperties}
    >
      <div className="th" role="row">
        {heads.map((head, at) => (
          <span key={at} role="columnheader">
            {head}
          </span>
        ))}
      </div>
      {children}
    </div>
  );
}

/** A table's row. `lit` flashes one that has just come. */
export function Tr({
  lit,
  soft,
  children,
}: {
  lit?: boolean;
  soft?: boolean;
  children?: ReactNode;
}) {
  return (
    <div
      className={["tr", lit && "lit", soft && "soft"].filter(Boolean).join(" ")}
      role="row"
    >
      {children}
    </div>
  );
}

/** A cell. `wide` ones are left out on a phone; `end` holds what can be done. */
export function Td({
  wide,
  end,
  children,
}: {
  wide?: boolean;
  end?: boolean;
  children?: ReactNode;
}) {
  return (
    <div className={end ? "td end" : wide ? "td wide" : "td"} role="cell">
      {children}
    </div>
  );
}

/** Who or what a row is about: its picture, its name, and a line under it. `phone` is said only where the other cells are left out. */
export function Who({
  lead,
  name,
  detail,
  phone,
}: {
  lead?: ReactNode;
  name: ReactNode;
  detail?: ReactNode;
  phone?: ReactNode;
}) {
  return (
    <span className="who">
      {lead}
      <span className="nm">
        <b>{name}</b>
        {detail ? <small>{detail}</small> : null}
        {phone ? <small className="ph">{phone}</small> : null}
      </span>
    </span>
  );
}

/** How something stands, as a dot and a word. */
export function Seal({
  tone,
  children,
}: {
  tone?: "ok" | "wait" | "bad";
  children: ReactNode;
}) {
  return (
    <span className={tone ? `seal ${tone}` : "seal"}>
      <i />
      {children}
    </span>
  );
}

export function Search({
  label,
  value,
  onChange,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <label className="search">
      <IconSearch size={14} />
      <input
        type="search"
        aria-label={label}
        placeholder={label}
        value={value}
        onChange={(event) => onChange(event.target.value)}
      />
    </label>
  );
}

/** One thing to know before reading the page, and what can be done about it. */
export function Notice({
  icon,
  tone,
  title,
  actions,
  children,
}: {
  icon: ReactNode;
  tone?: "cold" | "bad" | "ok";
  title: ReactNode;
  actions?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className={tone ? `notice ${tone}` : "notice"} data-quiet="">
      {icon}
      <div>
        <b>{title}</b>
        {children ? <p>{children}</p> : null}
      </div>
      {actions ? <div className="do">{actions}</div> : null}
    </div>
  );
}

/** What a list says before it holds anything. */
export function Nothing({
  title,
  actions,
  children,
}: {
  title: ReactNode;
  actions?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="nothing">
      <h2>{title}</h2>
      {children ? <p>{children}</p> : null}
      {actions ? <div className="do">{actions}</div> : null}
    </div>
  );
}

/** A choice and what it is for: the words on the left, the control on the right. */
export function Setting({
  name,
  detail,
  children,
}: {
  name: ReactNode;
  detail?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <section className="setting">
      <div className="sl">
        <h2>{name}</h2>
        {detail ? <p>{detail}</p> : null}
      </div>
      <div className="sr">{children}</div>
    </section>
  );
}

/** Numbers side by side under small captions, such as a bill's. */
export function Figures({ children }: { children: ReactNode }) {
  return <div className="figures">{children}</div>;
}

export function Figure({
  label,
  small,
  detail,
  children,
}: {
  label: ReactNode;
  small?: boolean;
  detail?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className={small ? "figure sm" : "figure"}>
      <span className="cap">{label}</span>
      <span className="sum">{children}</span>
      {detail ? <span className="note">{detail}</span> : null}
    </div>
  );
}

/** A thing said at a page's foot, with the one action that answers it. */
export function Ending({
  title,
  actions,
  children,
}: {
  title: ReactNode;
  actions?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <section className="closing">
      <div>
        <h2>{title}</h2>
        {children ? <p>{children}</p> : null}
      </div>
      {actions ? <div className="do">{actions}</div> : null}
    </section>
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
    <div className={failed ? "toast bad" : "toast"} role="status">
      {failed ? <IconClose size={17} /> : <IconCircleCheck size={17} />}
      <span>{failed ? "Undo didn't go through" : note.text}</span>
      {note.undo ? (
        <Txt onClick={() => void undo()} disabled={busy}>
          {failed ? "Try again" : "Undo"}
        </Txt>
      ) : null}
    </div>
  );
}
