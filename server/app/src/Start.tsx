import { useUser } from "@clerk/react";
import type {
  BillingCatalog,
  Org,
  OrgOffer,
  TeamAccess,
} from "@sikemux/protocol";
import { IconBack, IconChevron, IconLink, IconPlus } from "@shared/ui/Icons";
import { type MouseEvent, type ReactNode, useEffect, useState } from "react";

import { problemText } from "./api.ts";
import { OrgAvatar } from "./Avatar.tsx";
import { orgSvg, personSvg } from "./avatarSvg.ts";
import {
  FREE_DURING_PREVIEW,
  NewTeam,
  NOT_OPEN,
  useCheckout,
  useClient,
} from "./Billing.tsx";
import { useCheer } from "./Field.tsx";
import { BringSeat } from "./Fold.tsx";
import { Foot } from "./Frame.tsx";
import { brandMark, iconMark } from "./marks.ts";
import { money, priceOf, productOf } from "./money.ts";
import { HOSTS, JOIN, orgPath } from "./navigation.ts";
import {
  Btn,
  btnClass,
  Field,
  Hero,
  Problem,
  Row,
  Rows,
  Tile,
  Txt,
  useAction,
} from "./ui.tsx";

type Follow = (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
type Joined = (orgId: string) => Promise<void>;

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
  const first = orgs.length === 0;
  const unit = catalog && priceOf(productOf(catalog, "team"), "month");
  const perSeat = unit && `${money(unit, { cents: false })} a seat a month.`;
  const preview = team === "preview";
  const own =
    preview && !staff
      ? orgs.find((org) => org.role === "owner" && org.preview)
      : undefined;
  const back = first ? (
    <Txt onClick={() => setNaming(false)}>
      <IconBack className="ic" /> Back
    </Txt>
  ) : (
    <a
      className="txt"
      href={orgPath(orgs[0].id)}
      onClick={follow(orgPath(orgs[0].id))}
    >
      <IconBack className="ic" /> Back
    </a>
  );

  if (own)
    return (
      <Hero
        title="One org each, for now"
        lede={`The preview gives each person one org, and you made ${own.name}.`}
        mark={{ svg: orgSvg(own.id, "hero") }}
        scene="blue"
        tone="waiting"
        back={back}
        foot={<Foot />}
      >
        <div className="do">
          <a
            className={btnClass({})}
            href={orgPath(own.id)}
            onClick={follow(orgPath(own.id))}
          >
            Go to {own.name}
          </a>
          <a
            className={btnClass({ quiet: true })}
            href={JOIN}
            onClick={follow(JOIN)}
          >
            Join another with a link
          </a>
        </div>
        <Others offers={offers} onJoined={onJoined} />
      </Hero>
    );

  if (first && !naming)
    return (
      <Hero
        title={`Welcome${user?.firstName ? `, ${user.firstName}` : ""}`}
        lede={
          offers.length > 0
            ? `${offers[0].name} uses your work address. You can join now.`
            : "Sikemux works on your own. Bring others in when you want to."
        }
        mark={user ? { svg: personSvg(user.id, "hero") } : brandMark}
        scene="blue"
        foot={<Foot />}
      >
        <div className="two">
          {offers.map((offer) => (
            <OfferCard key={offer.orgId} offer={offer} onJoined={onJoined} />
          ))}
          <button type="button" className="ch" onClick={() => setNaming(true)}>
            <IconPlus className="ic" />
            <b>{offers.length > 0 ? "Start your own org" : "Start an org"}</b>
            <span>
              {offers.length > 0
                ? "You can be in as many as you like."
                : preview
                  ? "Free for up to 20 people in the preview."
                  : (perSeat ?? "Seats are paid by the month.")}
            </span>
            <em>
              Name it <IconChevron className="ic" />
            </em>
          </button>
          <a className="ch" href={JOIN} onClick={follow(JOIN)}>
            <IconLink className="ic" />
            <b>Join with a link</b>
            <span>Paste the link somebody sent you.</span>
            <em>
              Paste it <IconChevron className="ic" />
            </em>
          </a>
        </div>
        <p className="note">
          <a className="txt" href={HOSTS} onClick={follow(HOSTS)}>
            Just me for now
          </a>
        </p>
        <Moved />
      </Hero>
    );

  return (
    <Hero
      title="Name your org"
      lede={
        preview
          ? "Free for up to 20 people. You can be in others too."
          : "You can be in others too."
      }
      mark={iconMark("org")}
      scene="blue"
      back={back}
      foot={<Foot />}
    >
      {preview ? (
        <PreviewForm perSeat={perSeat} onJoined={onJoined} />
      ) : (
        <SoldForm
          catalog={catalog}
          canBuy={canBuy}
          onCancel={() =>
            first ? setNaming(false) : location.assign(orgPath(orgs[0].id))
          }
        />
      )}
      <Problem>{problem}</Problem>
      <Others offers={first ? [] : offers} onJoined={onJoined} />
    </Hero>
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
  onJoined,
}: {
  perSeat?: string;
  onJoined: Joined;
}) {
  const client = useClient();
  const cheer = useCheer();
  const action = useAction();
  const [name, setName] = useState("");
  return (
    <>
      <form
        className="do"
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
        <Field
          aria-label="Org name"
          placeholder="Your org&rsquo;s name"
          value={name}
          maxLength={40}
          autoFocus
          onChange={(event) => setName(event.target.value)}
        />
        <Btn type="submit" busy={action.busy} disabled={!name.trim()}>
          Start the org
        </Btn>
      </form>
      <p className="fine">
        {perSeat ? `${perSeat} ` : ""}
        {FREE_DURING_PREVIEW}.
      </p>
      <Problem>{action.problem}</Problem>
    </>
  );
}

function SoldForm({
  catalog,
  canBuy,
  onCancel,
}: {
  catalog?: BillingCatalog;
  canBuy: boolean;
  onCancel: () => void;
}) {
  const checkout = useCheckout();
  if (!catalog) return null;
  if (!canBuy) return <p className="fine">{NOT_OPEN}.</p>;
  return <NewTeam catalog={catalog} checkout={checkout} onCancel={onCancel} />;
}

function useJoin(offer: OrgOffer, onJoined: Joined) {
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
  return { ...action, join };
}

function offerWords(offer: OrgOffer) {
  return offer.seatFree
    ? `Open to everyone at ${offer.domain}. You'd join as Member, on one of its free seats.`
    : `Open to everyone at ${offer.domain}, but every seat is taken. Ask its owner to add one.`;
}

function OfferCard({ offer, onJoined }: { offer: OrgOffer; onJoined: Joined }) {
  const { join, busy, problem } = useJoin(offer, onJoined);
  return (
    <>
      <button
        type="button"
        className="ch"
        disabled={!offer.seatFree || busy}
        data-busy={busy || undefined}
        onClick={join}
      >
        <OrgAvatar id={offer.orgId} size={30} />
        <b>{offer.name}</b>
        <span>{offerWords(offer)}</span>
        {offer.seatFree ? (
          <em>
            {busy ? "Joining" : "Join"} <IconChevron className="ic" />
          </em>
        ) : null}
      </button>
      <Problem>{problem}</Problem>
    </>
  );
}

function OfferRow({ offer, onJoined }: { offer: OrgOffer; onJoined: Joined }) {
  const { join, busy, problem } = useJoin(offer, onJoined);
  return (
    <Row
      lead={<OrgAvatar id={offer.orgId} size={28} />}
      name={offer.name}
      detail={offerWords(offer)}
      wrap
      end={
        <Btn small disabled={!offer.seatFree} busy={busy} onClick={join}>
          Join
        </Btn>
      }
    >
      <Problem>{problem}</Problem>
    </Row>
  );
}

/** Ways into an org other than starting one, under the form. */
function Others({
  offers,
  onJoined,
}: {
  offers: OrgOffer[];
  onJoined: Joined;
}) {
  return (
    <>
      {offers.length > 0 ? (
        <Rows>
          {offers.map((offer) => (
            <OfferRow key={offer.orgId} offer={offer} onJoined={onJoined} />
          ))}
        </Rows>
      ) : null}
      <Moved />
    </>
  );
}

function Moved() {
  return (
    <Rows>
      <Help
        icon={<IconLink className="ic" />}
        name="Been given an invite link?"
        detail="Open it while signed in here and you join that org. Nothing is mailed, so ask the owner for one."
      />
      <Help
        icon={<IconPlus className="ic" />}
        name="Hold a seat on another account?"
        detail="Sign in with that account once and its seat moves here. It must hold nothing but the seat."
        end={<BringSeat />}
      />
    </Rows>
  );
}

function Help({
  icon,
  name,
  detail,
  end,
}: {
  icon: ReactNode;
  name: string;
  detail: string;
  end?: ReactNode;
}) {
  return (
    <Row
      lead={<Tile>{icon}</Tile>}
      name={name}
      detail={detail}
      end={end}
      wrap
    />
  );
}
