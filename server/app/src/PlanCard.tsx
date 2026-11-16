import type { Money } from "@sikemux/protocol";
import { IconCheck } from "@shared/ui/Icons";
import type { ReactNode } from "react";

import { money } from "./money.ts";

export type CardLine = { key: string; text: ReactNode };

export function PlanCard({
  name,
  aside,
  price,
  per,
  tag,
  plus,
  lines,
  chosen,
  children,
}: {
  name: ReactNode;
  aside?: ReactNode;
  price?: Money;
  per?: string;
  tag?: ReactNode;
  plus?: string;
  lines: CardLine[];
  chosen?: boolean;
  children?: ReactNode;
}) {
  return (
    <div className="plan-card" data-chosen={chosen || undefined}>
      <h3>
        {name}
        {aside ? <span className="aside">{aside}</span> : null}
      </h3>
      {price ? (
        <div className="price">
          {money(price, { cents: false })}
          {per ? <small> {per}</small> : null}
        </div>
      ) : null}
      {tag ? <div className="tag">{tag}</div> : null}
      {plus ? <div className="plus">{plus}</div> : null}
      <ul className="features">
        {lines.map((line) => (
          <li key={line.key}>
            <IconCheck size={13} />
            <span>{line.text}</span>
          </li>
        ))}
      </ul>
      {children ? <div className="plan-card-foot">{children}</div> : null}
    </div>
  );
}
