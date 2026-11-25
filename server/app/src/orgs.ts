import type {
  Org,
  OrgOffer,
  OrgPolicy,
  OrgRole,
  TeamAccess,
} from "@sikemux/protocol";
import { useAuth } from "@clerk/react";
import { useCallback, useEffect, useEffectEvent, useState } from "react";

import { problemText } from "./api.ts";
import { useClient } from "./Billing.tsx";
import { loadLogos } from "./logos.ts";
import type { OrgPage } from "./navigation.ts";

const LAST_KEY = "sikemux.org";

export const ROLE_WORDS: Record<OrgRole, string> = {
  owner: "Owner",
  admin: "Admin",
  user: "Member",
};

/** Whether someone may manage the org's people and settings. */
export const runs = (org: Org) => org.role === "owner" || org.role === "admin";

/** An org whose plan ended keeps everything, but takes nobody new and changes nothing. */
export const frozen = (org: Org) => org.seats.limit === 0;

/** Whether a frozen org's plan ended and can be renewed. Otherwise Team itself is shut to the person. */
export const renewable = (org: Org, team: TeamAccess) =>
  frozen(org) && team === "sold";

/** Whether the org takes changes from someone Team is open to this way. */
export const changeable = (org: Org, team: TeamAccess) =>
  !frozen(org) && (team === "preview" || team === "sold");

/** Whether someone with `actor`'s role may change, remove or invite someone with `role`, as the API decides it. */
export function manages(actor: OrgRole, role: OrgRole): boolean {
  return actor === "owner" || (actor === "admin" && role !== "owner");
}

/** Whether the org's page is one the API answers someone with this role. `billed` is whether billing answers them at all. */
export function seesPage(org: Org, page: OrgPage, billed: boolean): boolean {
  if (["people", "groups", "policy", "details"].includes(page)) return true;
  return page === "billing" ? billed && org.role === "owner" : runs(org);
}

export type Orgs = {
  /** Unset until read. */
  list?: Org[];
  problem?: string;
  reload: () => Promise<void>;
};

/** The orgs the person is in, read again whenever `key` changes. */
export function useOrgs(ready: boolean, key: string): Orgs {
  const client = useClient();
  const { getToken } = useAuth();
  const [state, setState] = useState<Omit<Orgs, "reload">>({});
  const read = useCallback(async () => {
    try {
      const { orgs } = await (await client("see your orgs")).listOrgs();
      loadLogos(orgs, getToken);
      return { list: orgs };
    } catch (error) {
      return { problem: `${problemText(error)} Reload the page to try again.` };
    }
  }, [client, getToken]);
  const reload = useCallback(async () => {
    const next = await read();
    setState(({ list }) => ({ list, ...next }));
  }, [read]);
  useEffect(() => {
    if (!ready) return;
    let current = true;
    void read().then(
      (next) => current && setState(({ list }) => ({ list, ...next })),
    );
    return () => {
      current = false;
    };
  }, [ready, key, read]);
  return { ...state, reload };
}

/** The org this tab last showed, which a link that names no org opens. */
export function lastOrg(orgs: Org[]): Org | undefined {
  const id = sessionStorage.getItem(LAST_KEY);
  return orgs.find((org) => org.id === id) ?? orgs[0];
}

export const keepLastOrg = (id: string) => sessionStorage.setItem(LAST_KEY, id);

/** The orgs whose email domains would take the person in. */
export function useOffers(ready: boolean) {
  const client = useClient();
  const [offers, setOffers] = useState<OrgOffer[]>([]);
  const read = useCallback(async () => {
    try {
      const routes = await client("see orgs you can join");
      return (await routes.listOrgOffers()).offers;
    } catch {
      return [];
    }
  }, [client]);
  const reload = useCallback(async () => setOffers(await read()), [read]);
  useEffect(() => {
    if (!ready) return;
    let current = true;
    void read().then((found) => current && setOffers(found));
    return () => {
      current = false;
    };
  }, [ready, read]);
  return { offers: ready ? offers : [], reload };
}

/** Loads what a page shows, again whenever `key` changes or `again` is called. */
export function useLoad<T>(load: () => Promise<T>, key: string) {
  const [state, setState] = useState<{ data?: T; problem?: string }>({});
  const [round, setRound] = useState(0);
  const loadNow = useEffectEvent(load);
  useEffect(() => {
    let current = true;
    loadNow().then(
      (data) => current && setState({ data }),
      (error: unknown) => current && setState({ problem: problemText(error) }),
    );
    return () => {
      current = false;
    };
  }, [key, round]);
  const again = useCallback(() => setRound((last) => last + 1), []);
  return { ...state, again };
}

export const RULE_WORDS: Record<OrgPolicy["ruleAdding"], string> = {
  every_member: "Every member",
  admins_approve: "Admins approve first",
};

export const possessive = (name: string) =>
  name.endsWith("s") ? `${name}'` : `${name}'s`;
