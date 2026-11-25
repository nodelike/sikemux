import { useUser } from "@clerk/react";
import type {
  BillingCatalog,
  Org,
  OrgOffer,
  TeamAccess,
} from "@sikemux/protocol";
import {
  IconLink,
  IconMinus,
  IconPlus,
  IconUser,
  IconUsers,
} from "@shared/ui/Icons";
import { type MouseEvent, useEffect, useState } from "react";

import "./account.css";
import { problemText } from "./api.ts";
import { OrgAvatar } from "./Avatar.tsx";
import { NOT_OPEN, useCheckout } from "./BillingShared.tsx";
import { useClient } from "./client.ts";
import { useCheer } from "./Field.tsx";
import { BringSeat } from "./Fold.tsx";
import { iconMark } from "./marks.ts";
import { money, PER, priceOf, productOf, times } from "./money.ts";
import { HOSTS, JOIN, orgPath } from "./navigation.ts";
import { Btn, btnClass, Notice, Page, Problem, useAction } from "./ui.tsx";

type Follow = (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
type Joined = (orgId: string) => Promise<void>;

const NAME_MAX = 64;

/** Starting an org of one's own, or joining one that takes the person in. */
export function StartPage({
  team,
  staff,
  orgs,
  offers,
  follow,
  onJoined,
}: {
  team: TeamAccess;
  staff: boolean;
  orgs: Org[];
  offers: OrgOffer[];
  follow: Follow;
  onJoined: Joined;
}) {
  const { user } = useUser();
  const [naming, setNaming] = useState(false);
  const { catalog, canBuy, problem } = usePrices(team);
  const unit = catalog && priceOf(productOf(catalog, "team"), "month");
  const perSeat = unit && `${money(unit, { cents: false })} a seat a month.`;
  const preview = team === "preview";
  const own =
    preview && !staff
      ? orgs.find((org) => org.role === "owner" && org.preview)
      : undefined;
  const skipTo = orgs[0] ? orgPath(orgs[0].id) : HOSTS;

  if (naming && !own)
    return (
      <div className="start-page">
        <Page
          title="Name your org"
          lede="You can change it later."
          mark={iconMark("org")}
          scene="you"
        >
          {preview ? (
            <PreviewForm
              perSeat={perSeat}
              onBack={() => setNaming(false)}
              onJoined={onJoined}
            />
          ) : (
            <SoldForm
              catalog={catalog}
              canBuy={canBuy}
              problem={problem}
              onBack={() => setNaming(false)}
            />
          )}
        </Page>
      </div>
    );

  return (
    <div className="start-page">
      <Page
        title={
          own
            ? "Orgs"
            : `Welcome${user?.firstName ? `, ${user.firstName}` : ""}`
        }
        lede="Sikemux works on your own. Bring others in when you want to."
        notice={
          own ? (
            <Notice
              icon={<IconUsers size={16} />}
              title="One org each, for now"
              actions={
                <a
                  className={btnClass({ quiet: true, small: true })}
                  href={orgPath(own.id)}
                  onClick={follow(orgPath(own.id))}
                >
                  Open {own.name}
                </a>
              }
            >
              You already started {own.name}.
            </Notice>
          ) : undefined
        }
        mark={iconMark("org")}
        scene="you"
      >
        <div className="start-list">
          {offers.map((offer) => (
            <OfferPath key={offer.orgId} offer={offer} onJoined={onJoined} />
          ))}
          <button
            type="button"
            className="start-path"
            disabled={Boolean(own)}
            onClick={() => setNaming(true)}
          >
            <span className="start-ic">
              <IconPlus size={20} />
            </span>
            <span className="start-copy">
              <b>{offers.length > 0 ? "Start your own org" : "Start an org"}</b>
              <small>
                {preview
                  ? "Free for up to 20 people in the preview."
                  : (perSeat ?? "Seats are paid by the month.")}
              </small>
            </span>
            <em className="start-pill">Start</em>
          </button>
          <a className="start-path" href={JOIN} onClick={follow(JOIN)}>
            <span className="start-ic">
              <IconLink size={20} />
            </span>
            <span className="start-copy">
              <b>Join with a link</b>
              <small>Paste the link somebody sent you.</small>
            </span>
            <em className="start-pill">Paste</em>
          </a>
          <a className="start-path" href={skipTo} onClick={follow(skipTo)}>
            <span className="start-ic">
              <IconUser size={20} />
            </span>
            <span className="start-copy">
              <b>Just me for now</b>
              <small>Sikemux works on your own.</small>
            </span>
            <em className="start-pill">Skip</em>
          </a>
        </div>
        <p className="start-fine">
          Hold a seat on another account? Sign in with that account once and its
          seat moves here. It must hold nothing but the seat. <BringSeat />
        </p>
      </Page>
    </div>
  );
}

/** Team's price, and whether the person may buy it, which only checkout needs to know. */
function usePrices(team: TeamAccess) {
  const client = useClient();
  const [state, setState] = useState<{
    catalog?: BillingCatalog;
    canBuy: boolean;
    problem?: string;
  }>({ canBuy: false });
  const sold = team === "sold";
  useEffect(() => {
    let current = true;
    void (async () => {
      try {
        const routes = await client("see Team");
        const [catalog, overview] = await Promise.all([
          routes.getBillingCatalog(),
          sold ? routes.getBilling() : undefined,
        ]);
        if (current) setState({ catalog, canBuy: overview?.canBuy ?? false });
      } catch (error) {
        if (current && sold)
          setState({
            canBuy: false,
            problem: `${problemText(error)} Reload the page to try again.`,
          });
      }
    })();
    return () => {
      current = false;
    };
  }, [client, sold]);
  return state;
}

function PreviewForm({
  perSeat,
  onBack,
  onJoined,
}: {
  perSeat?: string;
  onBack: () => void;
  onJoined: Joined;
}) {
  const client = useClient();
  const cheer = useCheer();
  const action = useAction();
  const [name, setName] = useState("");
  return (
    <form
      className="start-name"
      onSubmit={(event) => {
        event.preventDefault();
        if (!name.trim() || action.busy) return;
        void action.run(
          async () => {
            const routes = await client("start a Team");
            const org = await routes.createOrg({ name: name.trim() });
            cheer();
            await onJoined(org.id);
          },
          (error) => `${problemText(error)} No org was made.`,
        );
      }}
    >
      <input
        className="start-name-in"
        aria-label="Org name"
        placeholder="Your org's name"
        value={name}
        maxLength={40}
        autoFocus
        disabled={action.busy}
        onChange={(event) => setName(event.target.value)}
      />
      <p className="start-hint">
        Free for up to 20 people in the preview.
        {perSeat ? ` After that, ${perSeat}` : ""}
      </p>
      <Problem>{action.problem}</Problem>
      <div className="start-acts">
        <Btn type="submit" busy={action.busy} disabled={!name.trim()}>
          {action.busy ? "Making it" : "Make the org"}
        </Btn>
        <Btn quiet disabled={action.busy} onClick={onBack}>
          Back
        </Btn>
      </div>
    </form>
  );
}

function SoldForm({
  catalog,
  canBuy,
  problem,
  onBack,
}: {
  catalog?: BillingCatalog;
  canBuy: boolean;
  problem?: string;
  onBack: () => void;
}) {
  const checkout = useCheckout();
  const [name, setName] = useState("");
  const [seats, setSeats] = useState(3);
  const [createdOrgId, setCreatedOrgId] = useState<string | null>(null);
  if (problem)
    return (
      <div className="start-name">
        <Problem>{problem}</Problem>
        <div className="start-acts">
          <Btn quiet onClick={onBack}>
            Back
          </Btn>
        </div>
      </div>
    );
  if (!catalog) return null;
  if (!canBuy)
    return (
      <div className="start-name">
        <p className="start-hint">{NOT_OPEN}.</p>
        <div className="start-acts">
          <Btn quiet onClick={onBack}>
            Back
          </Btn>
        </div>
      </div>
    );
  const team = productOf(catalog, "team");
  const unit = priceOf(team, "month");
  const maxSeats = team?.maxQuantity ?? 1000;
  return (
    <form
      className="start-name"
      onSubmit={(event) => {
        event.preventDefault();
        if (!name.trim() || checkout.busy) return;
        void checkout.go(async (routes) => {
          let orgId = createdOrgId;
          if (!orgId) {
            const org = await routes.createOrg({ name: name.trim() });
            orgId = org.id;
            setCreatedOrgId(orgId);
          }
          return routes.startCheckout({
            product: "team",
            interval: "month",
            quantity: seats,
            orgId,
          });
        });
      }}
    >
      <input
        className="start-name-in"
        aria-label="Org name"
        placeholder="Your org's name"
        value={name}
        maxLength={NAME_MAX}
        autoFocus
        disabled={checkout.busy || createdOrgId !== null}
        onChange={(event) => setName(event.target.value)}
      />
      <p className="start-hint">People see it when you invite them.</p>
      <div className="start-buy">
        <span className="start-step">
          <button
            type="button"
            aria-label="Fewer seats"
            disabled={checkout.busy || seats <= 1}
            onClick={() => setSeats((now) => Math.max(1, now - 1))}
          >
            <IconMinus size={14} />
          </button>
          <b>{seats}</b>
          <button
            type="button"
            aria-label="More seats"
            disabled={checkout.busy || seats >= maxSeats}
            onClick={() => setSeats((now) => Math.min(maxSeats, now + 1))}
          >
            <IconPlus size={14} />
          </button>
        </span>
        <span className="start-price">
          <b>
            {unit
              ? `${money(times(unit, seats), { cents: false })} ${PER.month}`
              : `${seats} ${seats === 1 ? "seat" : "seats"}`}
          </b>
          {unit ? (
            <small>
              {seats} {seats === 1 ? "seat" : "seats"} at{" "}
              {money(unit, { cents: false })}
            </small>
          ) : null}
        </span>
      </div>
      <Problem>{checkout.problem}</Problem>
      <div className="start-acts">
        <Btn type="submit" busy={checkout.busy} disabled={!name.trim()}>
          Start Team
        </Btn>
        <Btn quiet disabled={checkout.busy} onClick={onBack}>
          Back
        </Btn>
      </div>
    </form>
  );
}

function OfferPath({ offer, onJoined }: { offer: OrgOffer; onJoined: Joined }) {
  const client = useClient();
  const cheer = useCheer();
  const action = useAction();
  const join = () =>
    void action.run(
      async () => {
        const org = await (
          await client("join the org")
        ).joinOrgByDomain({ orgId: offer.orgId });
        cheer();
        await onJoined(org.id);
      },
      (error) => `${problemText(error)} You did not join.`,
    );
  return (
    <>
      <button
        type="button"
        className="start-path big"
        disabled={!offer.seatFree || action.busy}
        onClick={join}
      >
        <OrgAvatar org={{ id: offer.orgId }} size={40} />
        <span className="start-copy">
          <b>{offer.name} uses your work address</b>
          <small>
            {offer.seatFree
              ? `Open to everyone at ${offer.domain}. You'd join as Member, on one of its free seats.`
              : "Every seat is taken. Ask its owner to add one."}
          </small>
        </span>
        <em className="start-pill">
          {!offer.seatFree ? "Full" : action.busy ? "Joining" : "Join"}
        </em>
      </button>
      <Problem>{action.problem}</Problem>
    </>
  );
}
