import { IconCircleCheck } from "@shared/ui/Icons";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from "react";

/** A labelled group of rows, the unit every settings page is built from. */
export function Section({
  label,
  count,
  aside,
  children,
}: {
  label: string;
  count?: number | string;
  aside?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="group">
      <h2>
        {label}
        {count !== undefined ? <span className="count">{count}</span> : null}
        {aside ? <span className="aside">{aside}</span> : null}
      </h2>
      {children}
    </section>
  );
}

export function Rows({
  busy,
  children,
}: {
  busy?: boolean;
  children?: ReactNode;
}) {
  return (
    <ul className="rows" aria-busy={busy || undefined}>
      {busy ? <li className="row placeholder" /> : children}
    </ul>
  );
}

/** A setting: what it is on the left, its value, and what can be done to it. */
export function Row({
  label,
  action,
  below,
  children,
}: {
  label: ReactNode;
  action?: ReactNode;
  below?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <li className="row setting">
      <span className="key-label">{label}</span>
      <span className="value">{children}</span>
      {action ? <span className="end">{action}</span> : null}
      {below}
    </li>
  );
}

/** A thing with a mark, a name and a line about it, such as a host or an invoice. */
export function ItemRow({
  glyph,
  name,
  detail,
  end,
  children,
}: {
  glyph: ReactNode;
  name: ReactNode;
  detail?: ReactNode;
  end?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <li className="row">
      <span className="glyph">{glyph}</span>
      <span className="name">{name}</span>
      {detail ? <span className="detail">{detail}</span> : null}
      {end ? <span className="end">{end}</span> : null}
      {children}
    </li>
  );
}

export function PageHead({
  title,
  children,
}: {
  title: string;
  children?: ReactNode;
}) {
  return (
    <>
      <h1>{title}</h1>
      {children ? <p className="lede">{children}</p> : null}
    </>
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
  onCancel,
  onConfirm,
}: {
  id?: string;
  children: ReactNode;
  problem?: string;
  busy: boolean;
  label: string;
  danger?: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Escape" && !busy) onCancel();
  };
  return (
    <div className="confirm" id={id} onKeyDown={onKeyDown}>
      {children}
      {problem ? (
        <p className="problem" role="alert">
          {problem}
        </p>
      ) : null}
      <div className="actions">
        <button
          type="button"
          className="button small"
          onClick={onCancel}
          disabled={busy}
          autoFocus
        >
          Cancel
        </button>
        <button
          type="button"
          className={danger ? "button small danger" : "button small primary"}
          onClick={onConfirm}
          disabled={busy}
          data-busy={busy || undefined}
        >
          {label}
        </button>
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

export function Segmented<Value extends string>({
  label,
  options,
  value,
  onChange,
}: {
  label: string;
  options: { value: Value; name: ReactNode }[];
  value: Value;
  onChange: (value: Value) => void;
}) {
  return (
    <span className="segmented" role="radiogroup" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          role="radio"
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

/** A short line at the foot of the page saying what just changed, with Undo when it can be taken back. */
export function useNotice() {
  const [note, setNote] = useState<Note>();
  const next = useRef(0);
  useEffect(() => {
    if (!note) return;
    const timer = setTimeout(() => setNote(undefined), 6000);
    return () => clearTimeout(timer);
  }, [note]);
  const show = useCallback(
    (text: string, undo?: () => Promise<void>) =>
      setNote({ id: ++next.current, text, undo }),
    [],
  );
  const notice = note ? (
    <Notice key={note.id} note={note} done={() => setNote(undefined)} />
  ) : null;
  return { notice, show };
}

function Notice({ note, done }: { note: Note; done: () => void }) {
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
    <div className="notice" role="status">
      <IconCircleCheck size={16} />
      <span>{failed ? "Undo didn't go through. Try again." : note.text}</span>
      {note.undo ? (
        <button
          type="button"
          className="undo"
          onClick={() => void undo()}
          disabled={busy}
        >
          Undo
        </button>
      ) : null}
    </div>
  );
}
