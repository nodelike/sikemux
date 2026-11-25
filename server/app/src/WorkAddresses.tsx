import type {
  OrgOffer,
  TeamAccess,
  WorkAddress,
  WorkAddressProven,
} from "@sikemux/protocol";
import { formatDay } from "@shared/lib/format";
import { IconShieldCheck } from "@shared/ui/Icons";
import { useEffect, useRef, useState } from "react";

import "./account.css";
import { problemText } from "./api.ts";
import { OrgAvatar } from "./Avatar.tsx";
import { useClient } from "./Billing.tsx";
import { GoogleMark } from "./icons.tsx";
import { WORK_ADDRESS } from "./navigation.ts";
import {
  Btn,
  Confirm,
  Problem,
  Row,
  Rows,
  Section,
  Tile,
  Txt,
  useAction,
  useToast,
} from "./ui.tsx";

/** Addresses the person proved through Google are theirs at a domain an org verified, so they can join it. */
export function WorkAddressesCard({ team }: { team: TeamAccess }) {
  const client = useClient();
  const action = useAction();
  const { run } = action;
  const say = useToast();
  const [working, setWorking] = useState<string>();
  const [removing, setRemoving] = useState<WorkAddress>();
  const [addresses, setAddresses] = useState<WorkAddress[]>();
  const [verifying, setVerifying] = useState(false);
  const [proven, setProven] = useState<WorkAddressProven>();
  const returned = useRef(false);
  const open = verifying && (team === "sold" || team === "preview");

  useEffect(() => {
    void (async () => {
      const list = await (
        await client("list work addresses")
      ).listWorkAddresses();
      setAddresses(list.workAddresses);
      setVerifying(list.verifying);
    })().catch(() => undefined);
  }, [client]);

  useEffect(() => {
    if (returned.current || location.pathname !== WORK_ADDRESS) return;
    returned.current = true;
    const back = new URLSearchParams(location.search);
    history.replaceState(null, "", "/");
    const code = back.get("code");
    const state = back.get("state");
    if (!code || !state) return;
    void run(async () => {
      const done = await (
        await client("verify the work address")
      ).finishWorkAddressVerify({ code, state });
      setProven(done);
      setAddresses((now) => [
        ...(now ?? []).filter((each) => each.id !== done.workAddress.id),
        done.workAddress,
      ]);
    }, problemText);
  }, [run, client]);

  const start = () => {
    setWorking("start");
    return action.run(async () => {
      const { url } = await (
        await client("verify a work address")
      ).startWorkAddressVerify();
      location.assign(url);
    }, problemText);
  };

  const remove = (address: WorkAddress) => {
    setWorking(address.id);
    return action.run(async () => {
      await (
        await client("remove the work address")
      ).removeWorkAddress(address.id);
      setAddresses((now) => now?.filter((each) => each.id !== address.id));
      if (proven?.workAddress.id === address.id) setProven(undefined);
      setRemoving(undefined);
      say(`Your address at ${address.domain} is no longer verified.`);
    }, problemText);
  };

  if (!addresses || (addresses.length === 0 && !open && !action.problem))
    return null;

  return (
    <>
      <Section
        label="Work addresses"
        count={addresses.length || undefined}
        aside={
          open ? (
            <Btn
              small
              quiet
              busy={action.busy && working === "start"}
              disabled={action.busy}
              onClick={() => void start()}
            >
              <GoogleMark />
              Verify a work address
            </Btn>
          ) : undefined
        }
        foot={
          addresses.length > 0
            ? "It never signs you in. We keep the domain, not the address."
            : "Through the Google account at your work domain. We keep the domain, not the address."
        }
      >
        {addresses.length === 0 ? (
          <p className="note">None. Verify one to find orgs at your work.</p>
        ) : (
          <Rows>
            {addresses.map((address) => (
              <Row
                key={address.id}
                lead={
                  <Tile>
                    <IconShieldCheck size={16} />
                  </Tile>
                }
                name={address.domain}
                detail={`Verified ${formatDay(address.provenAt)}${address.orgName ? ` · ${address.orgName}` : ""}`}
                end={
                  <Txt
                    aria-label={`Remove ${address.domain}`}
                    disabled={action.busy}
                    onClick={() => {
                      action.clear();
                      setRemoving(address);
                    }}
                  >
                    Remove
                  </Txt>
                }
              />
            ))}
          </Rows>
        )}
        {open && proven?.offer ? (
          <OfferBanner
            offer={proven.offer}
            sitting={proven.sitting}
            onJoined={(org) => {
              setProven(undefined);
              say(`You joined ${org.name}`);
            }}
          />
        ) : null}
        {!removing ? <Problem>{action.problem}</Problem> : null}
      </Section>

      {removing ? (
        <Confirm
          title={`Remove ${removing.domain}?`}
          label="Remove"
          danger
          busy={action.busy}
          problem={action.problem}
          onCancel={() => {
            action.clear();
            setRemoving(undefined);
          }}
          onConfirm={() => void remove(removing)}
        >
          <p>
            You can verify it again any time. Any org you already joined stays
            as it is.
          </p>
        </Confirm>
      ) : null}
    </>
  );
}

function OfferBanner({
  offer,
  sitting,
  onJoined,
}: {
  offer: OrgOffer;
  sitting: string | undefined;
  onJoined: (org: { name: string }) => void;
}) {
  const client = useClient();
  const action = useAction();
  const join = () =>
    action.run(
      async () => {
        await (
          await client("join the org")
        ).joinOrgByDomain({ orgId: offer.orgId, sitting });
        onJoined(offer);
      },
      (error) => `${problemText(error)} You did not join.`,
    );
  return (
    <>
      <div className="acc-offer">
        <OrgAvatar org={{ id: offer.orgId }} size={34} />
        <span>
          <b>
            {offer.name} uses {offer.domain}
          </b>
          <small>
            {offer.seatFree
              ? "You can join now, on a free seat."
              : "Every seat is taken. Ask its owner to add one."}
          </small>
        </span>
        <Btn
          small
          busy={action.busy}
          disabled={!offer.seatFree}
          onClick={() => void join()}
        >
          Join
        </Btn>
      </div>
      <Problem>{action.problem}</Problem>
    </>
  );
}
