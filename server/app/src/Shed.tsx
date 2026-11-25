import type {
  BillingCatalog,
  BillingInterval,
  BillingOverview,
  BillingProduct,
  Subscription,
} from "@sikemux/protocol";
import { IconCube, IconLock } from "@shared/ui/Icons";
import { useEffect, useState, type MouseEvent } from "react";

import {
  NOT_OPEN,
  ShedBox,
  Stepper,
  useCheckout,
  useClient,
  type Show,
} from "./Billing.tsx";
import { BilledSwitch } from "./BilledSwitch.tsx";
import { money, priceOf, productOf, SHED_WORDS, times } from "./money.ts";
import { BILLING } from "./navigation.ts";
import { Btn, btnClass, Section, Problem, Row, Rows, Tile } from "./ui.tsx";

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

/** On Hosts: a bought box that is not a host yet. */
export function ShedNotHost({
  follow,
}: {
  follow: (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
}) {
  const load = useShed();
  if (!load) return null;
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
        <Row
          lead={
            <Tile>
              <IconCube size={14} />
            </Tile>
          }
          name={`${name} not a host yet`}
          detail={
            one
              ? "Nothing is provisioned behind it. It joins this list once boxes ship."
              : "Nothing is provisioned behind them. They join this list once boxes ship."
          }
          end={
            <a
              className={btnClass({ small: true, quiet: true })}
              href={BILLING}
              onClick={follow(BILLING)}
            >
              Plan and billing
            </a>
          }
        />
      </Rows>
    </Section>
  );
}
