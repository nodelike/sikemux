import type { ReactNode } from "react";

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
  children,
}: {
  label: ReactNode;
  action?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <li className="row setting">
      <span className="key-label">{label}</span>
      <span className="value">{children}</span>
      {action ? <span className="end">{action}</span> : null}
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
