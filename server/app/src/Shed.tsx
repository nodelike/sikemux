import type {
  BillingCatalog,
  BillingInterval,
  BillingOverview,
  BillingProduct,
  Subscription,
} from "@sikemux/protocol";
import { formatDay } from "@shared/lib/format";
import { IconCube, IconInfo, IconLock } from "@shared/ui/Icons";
import { useEffect, useState, type MouseEvent } from "react";

import { problemText } from "./api.ts";
import {
  NOT_OPEN,
  Refunds,
  Stepper,
  useCheckout,
  useClient,
} from "./Billing.tsx";
import {
  money,
  priceOf,
  priceText,
  productOf,
  SHED_WORDS,
  times,
} from "./money.ts";
import { SHED } from "./navigation.ts";
import {
  ItemRow,
  PageHead,
  Rows,
  Section,
  Segmented,
  useNotice,
} from "./ui.tsx";

type Loaded = {
  catalog: BillingCatalog;
  overview: BillingOverview;
  readAt: number;
};

type Load =
  | { state: "loading" }
  | { state: "failed"; problem: string }
  | ({ state: "loaded" } & Loaded);

function useShed(version: number) {
  const client = useClient();
  const [load, setLoad] = useState<Load>({ state: "loading" });
  const [tick, setTick] = useState(0);
  useEffect(() => {
    let current = true;
    void (async () => {
      try {
        const routes = await client("see your Shed boxes");
        const [catalog, overview] = await Promise.all([
          routes.getBillingCatalog(),
          routes.getBilling({}),
        ]);
        if (current)
          setLoad({ state: "loaded", catalog, overview, readAt: Date.now() });
      } catch (error) {
        if (current) setLoad({ state: "failed", problem: problemText(error) });
      }
    })();
    return () => {
      current = false;
    };
  }, [version, tick, client]);
  return { load, reload: () => setTick((n) => n + 1) };
}

function boxesOf({ catalog, overview }: Loaded): Subscription[] {
  return overview.subscriptions.filter(
    (sub) =>
      sub.status !== "ended" &&
      productOf(catalog, sub.product)?.kind === "shed",
  );
}

export function ShedPage({ version }: { version: number }) {
  const { load, reload } = useShed(version);
  const { notice, show } = useNotice();
  return (
    <>
      <div className="staff-only">
        <IconLock size={13} />
        Staff only until boxes are provisioned.
      </div>
      <PageHead title="Shed">
        A Linux box we run, joined to your account. Buy one or several by the
        box. You sign in to Claude Code and Codex on it with your own plans.
      </PageHead>
      {load.state === "failed" ? (
        <p className="problem" role="alert">
          {load.problem}
        </p>
      ) : load.state === "loading" ? (
        <div className="groups">
          <Section label="Your boxes">
            <Rows busy />
          </Section>
        </div>
      ) : (
        <div className="groups">
          <Boxes {...load} />
          <Refunds
            subs={boxesOf(load)}
            readAt={load.readAt}
            catalog={load.catalog}
            reload={reload}
            show={show}
          />
          <Buy catalog={load.catalog} canBuy={load.overview.canBuy} />
        </div>
      )}
      {notice}
    </>
  );
}

function Boxes(loaded: Loaded) {
  const boxes = boxesOf(loaded);
  if (boxes.length === 0) return null;
  return (
    <Section label="Your boxes" count={boxes.length}>
      <Rows>
        {boxes.map((sub) => {
          const product = productOf(loaded.catalog, sub.product);
          const price = product ? priceText(product, sub.interval) : undefined;
          return (
            <ItemRow
              key={sub.id}
              glyph={<IconCube size={17} />}
              name={
                <>
                  {product?.name ?? sub.product}
                  {sub.quantity > 1 ? ` × ${sub.quantity}` : ""}
                  <span className="chip warn">Not provisioned</span>
                </>
              }
              detail={[`Bought ${formatDay(sub.startedAt, "long")}`, price]
                .filter(Boolean)
                .join(" · ")}
            />
          );
        })}
        <li className="row note">
          <span className="glyph">
            <IconInfo size={15} />
          </span>
          <span>
            Nothing is provisioned behind{" "}
            {boxes.length === 1 && boxes[0]?.quantity === 1
              ? "this box"
              : "these boxes"}{" "}
            yet: no machine runs, and nothing is used. Boxes join your Hosts the
            day they ship.
          </span>
        </li>
      </Rows>
    </Section>
  );
}

function Buy({
  catalog,
  canBuy,
}: {
  catalog: BillingCatalog;
  canBuy: boolean;
}) {
  const checkout = useCheckout();
  const [interval, setInterval] = useState<BillingInterval>("month");
  const [picked, setPicked] = useState<{
    product: BillingProduct;
    count: number;
  }>();
  const boxes = catalog.products.filter((product) => product.kind === "shed");
  if (boxes.length === 0) return null;
  const monthsPaid = monthsForAYear(boxes[0]);
  const unit = picked ? priceOf(picked.product, interval) : undefined;

  return (
    <Section
      label="Buy boxes"
      aside={
        <Segmented
          label="Billed"
          value={interval}
          onChange={setInterval}
          options={[
            { value: "month", name: "Monthly" },
            {
              value: "year",
              name: (
                <>
                  Yearly
                  {monthsPaid ? (
                    <em className="save">{monthsPaid} months' price</em>
                  ) : null}
                </>
              ),
            },
          ]}
        />
      }
    >
      <Rows>
        {boxes.map((product) => {
          const price = priceOf(product, interval);
          const words = SHED_WORDS[product.id];
          return (
            <ItemRow
              key={product.id}
              glyph={<IconCube size={17} />}
              name={
                <>
                  {product.name}
                  {words ? <span className="quiet"> · {words.tag}</span> : null}
                </>
              }
              detail={words?.specs.slice(0, 2).join(" · ")}
              end={
                <>
                  {price ? (
                    <span>
                      <span className="amount">
                        {money(price, { cents: false })}
                      </span>
                      <span className="quiet"> a box a {interval}</span>
                    </span>
                  ) : null}
                  <Stepper
                    value={picked?.product.id === product.id ? picked.count : 0}
                    min={0}
                    max={product.maxQuantity}
                    onChange={(count) =>
                      setPicked(count > 0 ? { product, count } : undefined)
                    }
                  />
                </>
              }
            />
          );
        })}
        <ItemRow
          glyph={null}
          name={
            picked ? `${picked.count} × ${picked.product.name}` : "Adding none"
          }
          detail={
            picked && unit
              ? `${money(times(unit, picked.count))} a ${interval}. One size at a time.`
              : "Change a count to buy."
          }
          end={
            <button
              type="button"
              className="button primary"
              disabled={!canBuy || !picked || !unit || checkout.busy}
              data-busy={checkout.busy || undefined}
              title={canBuy ? undefined : NOT_OPEN}
              onClick={() => {
                if (!picked) return;
                void checkout.go((routes) =>
                  routes.startCheckout({
                    product: picked.product.id,
                    interval,
                    quantity: picked.count,
                  }),
                );
              }}
            >
              {canBuy ? "Buy" : NOT_OPEN}
            </button>
          }
        >
          {checkout.problem ? (
            <p className="problem row-problem" role="alert">
              {checkout.problem}
            </p>
          ) : null}
        </ItemRow>
      </Rows>
    </Section>
  );
}

function monthsForAYear(product?: BillingProduct): number {
  const month = priceOf(product, "month");
  const year = priceOf(product, "year");
  return month && year ? Math.round(year.amount / month.amount) : 0;
}

/** On Free's plan page: where a Shed box can be bought. */
export function ShedOffer({
  catalog,
  follow,
}: {
  catalog: BillingCatalog;
  follow: (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
}) {
  const lowest = catalog.products
    .filter((product) => product.kind === "shed")
    .flatMap((product) => priceOf(product, "month") ?? [])
    .sort((a, b) => a.amount - b.amount)[0];
  if (!lowest) return null;
  return (
    <Section label="Shed">
      <Rows>
        <ItemRow
          glyph={<IconCube size={16} />}
          name="Shed boxes come on any plan, Free included"
          detail={`A Linux box we run, joined to your account. From ${money(lowest, { cents: false })} a box a month.`}
          end={
            <a className="button small" href={SHED} onClick={follow(SHED)}>
              See Shed
            </a>
          }
        />
      </Rows>
    </Section>
  );
}

/** On Hosts: a bought box that is not a host yet. */
export function ShedNotHost({
  follow,
}: {
  follow: (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
}) {
  const { load } = useShed(0);
  if (load.state !== "loaded") return null;
  const boxes = boxesOf(load);
  const first = boxes[0];
  if (!first) return null;
  const one = boxes.length === 1 && first.quantity === 1;
  const name = one
    ? `Your ${productOf(load.catalog, first.product)?.name ?? "Shed"} box is`
    : "Your Shed boxes are";
  return (
    <Section label="Shed">
      <Rows>
        <ItemRow
          glyph={<IconCube size={16} />}
          name={`${name} not a host yet`}
          detail={
            one
              ? "Nothing is provisioned behind it. It joins this list once boxes ship."
              : "Nothing is provisioned behind them. They join this list once boxes ship."
          }
          end={
            <a className="button small" href={SHED} onClick={follow(SHED)}>
              Open Shed
            </a>
          }
        />
      </Rows>
    </Section>
  );
}
