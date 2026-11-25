import { IconCheck, IconClock, IconMinus } from "@shared/ui/Icons";
import type { Money } from "@sikemux/protocol";
import type { ReactNode } from "react";

import { money } from "./money.ts";

export type BuiltState = "built" | "partly" | "no";

export type CardLine = {
  key: string;
  text: ReactNode;
  state?: BuiltState;
};

const STATE_ICON: Record<BuiltState, ReactNode> = {
  built: <IconCheck size={14} />,
  partly: <IconMinus size={14} />,
  no: <IconClock size={14} />,
};

export function PlanCard({
  name,
  price,
  summary,
  plus,
  lines,
  children,
}: {
  name: ReactNode;
  price?: Money;
  summary?: ReactNode;
  plus?: string;
  lines: CardLine[];
  children?: ReactNode;
}) {
  return (
    <section className="plan-col">
      <h2>{name}</h2>
      {price || summary ? (
        <p className="plan-price">
          {price ? <strong>{money(price, { cents: false })}</strong> : null}
          {price && summary ? " " : null}
          {summary}
        </p>
      ) : null}
      {plus ? <p className="plan-plus">{plus}</p> : null}
      <ul className="plan-feats">
        {lines.map((line) => {
          const state = line.state ?? "built";
          return (
            <li key={line.key} className={state}>
              {STATE_ICON[state]}
              <span>{line.text}</span>
            </li>
          );
        })}
      </ul>
      {children ? <div className="plan-foot">{children}</div> : null}
    </section>
  );
}
