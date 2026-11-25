import type {
  BillingCatalog,
  BillingInterval,
  BillingOverview,
  BillingProduct,
  Subscription,
} from "@sikemux/protocol";
import { formatDay } from "@shared/lib/format";
import { IconCube, IconLock } from "@shared/ui/Icons";
import { useEffect, useState, type MouseEvent } from "react";

import {
  NOT_OPEN,
  ShedBox,
  Stepper,
  useCheckout,
  useClient,
  type Show,
} from "./BillingShared.tsx";
import { useClient } from "./client.ts";
import { BilledSwitch } from "./BilledSwitch.tsx";
import { money, priceOf, productOf, SHED_WORDS, times } from "./money.ts";
import { BILLING } from "./navigation.ts";
import { Btn, btnClass, Section, Problem, Row, Rows, Tile } from "./ui.tsx";
import "./billing.css";

type Loaded = {
  catalog: BillingCatalog;
  overview: BillingOverview;
};

function useShed() {
  const client = useClient();
  const [load, setLoad] = useState<Loaded>();
  useEffect(() => {
    let current = true;
    void (async () => {
      try {
        const routes = await client("see your Shed boxes");
        const [catalog, overview] = await Promise.all([
          routes.getBillingCatalog(),
          routes.getBilling({}),
        ]);
        if (current) setLoad({ catalog, overview });
      } catch {
        if (current) setLoad(undefined);
      }
    })();
    return () => {
      current = false;
    };
  }, [client]);
  return load;
}

function boxesOf({ catalog, overview }: Loaded): Subscription[] {
  return overview.subscriptions.filter(
    (sub) =>
      sub.status !== "ended" &&
      productOf(catalog, sub.product)?.kind === "shed",
  );
}

/** Shed on Plan and billing: the boxes held, and for the person's own bill, the boxes for sale. */
export function ShedBoxes({
  catalog,
  bought,
  buying,
  canBuy,
  pays,
  reload,
  show,
}: {
  catalog: BillingCatalog;
  bought: Subscription[];
  buying: boolean;
  canBuy: boolean;
  pays: boolean;
  reload: () => void;
  show: Show;
}) {
  const boxes = catalog.products.filter((product) => product.kind === "shed");
  const forSale = buying && boxes.length > 0;
  const [interval, setInterval] = useState<BillingInterval>("month");
  if (!forSale && bought.length === 0) return null;
  const one = bought.length === 1 && bought[0]?.quantity === 1;
  return (
    <Section
      className="shed-boxes"
      label="Shed boxes"
      aside={
        forSale ? (
          <BilledSwitch
            interval={interval}
            onChange={setInterval}
            saving={boxes[0]}
          />
        ) : null
      }
      foot={
        <span className="staff-only">
          <IconLock size={12} />
          Staff only until boxes are provisioned.
          {bought.length > 0
            ? ` Nothing is provisioned behind ${one ? "this box" : "these boxes"} yet: no machine runs, and nothing is used. Boxes join your Hosts the day they ship.`
            : " Nothing runs behind a bought box yet."}
        </span>
      }
    >
      <Rows>
        {bought.map((sub) => (
          <ShedBox
            key={sub.id}
            sub={sub}
            catalog={catalog}
            canBuy={canBuy}
            pays={pays}
            reload={reload}
            show={show}
          />
        ))}
        {forSale ? (
          <Buy boxes={boxes} interval={interval} canBuy={canBuy} />
        ) : null}
      </Rows>
    </Section>
  );
}

function Buy({
  boxes,
  interval,
  canBuy,
}: {
  boxes: BillingProduct[];
  interval: BillingInterval;
  canBuy: boolean;
}) {
  const checkout = useCheckout();
  const [picked, setPicked] = useState<{
    product: BillingProduct;
    count: number;
  }>();
  const unit = picked ? priceOf(picked.product, interval) : undefined;
  const ready = canBuy && !!picked && !!unit;

  return (
    <>
      {boxes.map((product) => {
        const price = priceOf(product, interval);
        const words = SHED_WORDS[product.id];
        return (
          <Row
            key={product.id}
            wrap
            lead={
              <Tile>
                <IconCube size={14} />
              </Tile>
            }
            name={
              <>
                {product.name}
                {words ? <span className="soft">{words.tag}</span> : null}
              </>
            }
            detail={words?.line}
            end={
              <>
                {price ? (
                  <span>
                    <span className="amount">
                      {money(price, { cents: false })}
                    </span>{" "}
                    a box a {interval}
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
      <Row
        name={
          picked ? (
            `${picked.count} × ${picked.product.name}`
          ) : (
            <span className="soft">
              {canBuy ? "Pick how many boxes you want" : NOT_OPEN}
            </span>
          )
        }
        detail={
          picked
            ? `Billed ${interval === "month" ? "monthly" : "yearly"} · one size per checkout`
            : undefined
        }
        end={
          <>
            {picked && unit ? (
              <span>
                <span className="amount">
                  {money(times(unit, picked.count), { cents: false })}
                </span>{" "}
                a {interval}
              </span>
            ) : null}
            <Btn
              small
              quiet={!ready}
              disabled={!ready}
              busy={checkout.busy}
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
              Buy
            </Btn>
          </>
        }
      >
        {checkout.problem ? <Problem>{checkout.problem}</Problem> : null}
      </Row>
    </>
  );
}

/** On Hosts (staff only): bought Shed boxes, or the sizes for sale. */
export function ShedNotHost({
  follow,
}: {
  follow: (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
}) {
  const load = useShed();
  const checkout = useCheckout();
  const [buyingId, setBuyingId] = useState<string>();
  if (!load) return null;
  const bought = boxesOf(load);
  const aside = (
    <span className="note">Staff only until boxes are provisioned</span>
  );
  if (bought.length > 0) {
    const one = bought.length === 1 && bought[0]?.quantity === 1;
    return (
      <Section
        label="Shed"
        count={bought.length}
        aside={aside}
        foot={`Nothing is provisioned behind ${one ? "this box" : "these boxes"} yet. ${one ? "It joins" : "They join"} this list once boxes ship.`}
      >
        <Rows>
          {bought.map((sub) => {
            const product = productOf(load.catalog, sub.product);
            const words = SHED_WORDS[sub.product];
            const unit = priceOf(product, sub.interval);
            const cost = unit
              ? `${money(times(unit, sub.quantity), { cents: false })} a ${sub.interval}`
              : undefined;
            const detail = [
              words?.line,
              sub.quantity > 1 ? `${sub.quantity} boxes` : undefined,
              cost,
            ]
              .filter(Boolean)
              .join(" · ");
            return (
              <Row
                key={sub.id}
                lead={<IconCube size={22} className="host-ic" />}
                name={product?.name ?? sub.product}
                detail={detail || undefined}
                end={
                  <>
                    <span className="note">
                      Next bill {formatDay(sub.currentPeriodEnd, "long")}
                    </span>
                    <a
                      className={btnClass({ small: true, quiet: true })}
                      href={BILLING}
                      onClick={follow(BILLING)}
                    >
                      Change size
                    </a>
                  </>
                }
              />
            );
          })}
        </Rows>
      </Section>
    );
  }
  const boxes = load.catalog.products.filter(
    (product) => product.kind === "shed",
  );
  if (boxes.length === 0) return null;
  return (
    <Section label="Shed" aside={aside}>
      <div className="shed-grid">
        {boxes.map((product, index) => {
          const price = priceOf(product, "month");
          const words = SHED_WORDS[product.id];
          return (
            <div key={product.id}>
              <b>{product.name}</b>
              {words ? <span>{words.tag}</span> : null}
              {words ? <span>{words.line}</span> : null}
              {price ? (
                <strong>
                  {money(price, { cents: false })}
                  <small> a month</small>
                </strong>
              ) : null}
              <Btn
                small
                quiet={index !== 1}
                disabled={!load.overview.canBuy || !price}
                busy={buyingId === product.id && checkout.busy}
                onClick={() => {
                  setBuyingId(product.id);
                  void checkout.go((routes) =>
                    routes.startCheckout({
                      product: product.id,
                      interval: "month",
                      quantity: 1,
                    }),
                  );
                }}
              >
                {load.overview.canBuy ? "Buy" : NOT_OPEN}
              </Btn>
            </div>
          );
        })}
      </div>
      <Problem>{checkout.problem}</Problem>
    </Section>
  );
}
