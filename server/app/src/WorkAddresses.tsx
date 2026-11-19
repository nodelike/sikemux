import type { WorkAddress, WorkAddressProven } from "@sikemux/protocol";
import { formatDay } from "@shared/lib/format";
import { IconBriefcase, IconPlus } from "@shared/ui/Icons";
import { useEffect, useRef, useState } from "react";

import { problemText } from "./api.ts";
import { useClient } from "./Billing.tsx";
import { GoogleMark } from "./icons.tsx";
import { WORK_ADDRESS } from "./navigation.ts";
import { OfferRow } from "./OrgPages.tsx";
import { ItemRow, Rows, Section, useAction, useNotice } from "./ui.tsx";

/** Addresses the person proved through Google are theirs at a domain an org verified, so they can join it. */
export function WorkAddressesSection() {
  const client = useClient();
  const action = useAction();
  const { run } = action;
  const { notice, show } = useNotice();
  const [addresses, setAddresses] = useState<WorkAddress[]>();
  const [verifying, setVerifying] = useState(false);
  const [proven, setProven] = useState<WorkAddressProven>();
  const returned = useRef(false);

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

  const start = () =>
    action.run(async () => {
      const { url } = await (
        await client("verify a work address")
      ).startWorkAddressVerify();
      location.assign(url);
    }, problemText);

  const remove = (address: WorkAddress) =>
    action.run(async () => {
      await (
        await client("remove the work address")
      ).removeWorkAddress(address.id);
      setAddresses((now) => now?.filter((each) => each.id !== address.id));
      if (proven?.workAddress.id === address.id) setProven(undefined);
      show(`Your address at ${address.domain} is no longer verified.`);
    }, problemText);

  if (!addresses || (addresses.length === 0 && !verifying && !action.problem))
    return null;

  return (
    <Section label="Work addresses">
      <p className="section-lede">
        An address at a domain your org verified. It never signs in; it shows
        you're at that org so you can join it.
      </p>
      <Rows busy={action.busy}>
        {addresses.map((address) => (
          <ItemRow
            key={address.id}
            glyph={<IconBriefcase size={16} />}
            name={address.domain}
            detail={`${address.orgName ? `Verified for ${address.orgName}` : "Verified"} · ${formatDay(address.provenAt)}`}
            end={
              <button
                type="button"
                className="button small quiet-danger"
                disabled={action.busy}
                onClick={() => void remove(address)}
              >
                Remove
              </button>
            }
          />
        ))}
        {proven?.offer ? (
          <OfferRow
            offer={proven.offer}
            sitting={proven.sitting}
            onJoined={(org) => {
              setProven(undefined);
              show(`You joined ${org.name}`);
            }}
          />
        ) : null}
        {verifying ? (
          <ItemRow
            glyph={<IconPlus size={16} />}
            name="Verify a work address"
            detail="Sign in to the Google account at your work domain. Sikemux keeps the domain, not the address."
            end={
              <button
                type="button"
                className="button small"
                disabled={action.busy}
                onClick={() => void start()}
              >
                <GoogleMark />
                Verify with Google
              </button>
            }
          />
        ) : null}
      </Rows>
      {action.problem ? (
        <p className="problem" role="alert">
          {action.problem}
        </p>
      ) : null}
      {notice}
    </Section>
  );
}
