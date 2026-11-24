import type { Org, OrgOffer, OrgRole } from "@sikemux/protocol";
import { useCallback, useEffect, useEffectEvent, useState } from "react";

import { problemText } from "./api.ts";
import { useClient } from "./Billing.tsx";
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

/** Whether someone with `actor`'s role may change, remove or invite someone with `role`, as the API decides it. */
export function manages(actor: OrgRole, role: OrgRole): boolean {
  return actor === "owner" || (actor === "admin" && role !== "owner");
}

/** Whether the org's page is one the API answers someone with this role. */
export function seesPage(org: Org, page: OrgPage): boolean {
  if (page === "people" || page === "policy") return true;
  return page === "billing" ? org.role === "owner" : runs(org);
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
  const [state, setState] = useState<Omit<Orgs, "reload">>({});
  const reload = useCallback(async () => {
    try {
      const { orgs } = await (await client("see your orgs")).listOrgs();
      setState({ list: orgs });
    } catch (error) {
      const problem = `${problemText(error)} Reload the page to try again.`;
      setState(({ list }) => ({ list, problem }));
    }
  }, [client]);
  useEffect(() => {
    if (ready) void reload();
  }, [ready, key, reload]);
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
  const reload = useCallback(async () => {
    try {
      const routes = await client("see orgs you can join");
      setOffers((await routes.listOrgOffers()).offers);
    } catch {
      setOffers([]);
    }
  }, [client]);
  useEffect(() => {
    if (ready) void reload();
  }, [ready, reload]);
  return { offers, reload };
}

/** Loads what a page shows, again whenever `key` changes. */
export function useLoad<T>(load: () => Promise<T>, key: string) {
  const [state, setState] = useState<{ data?: T; problem?: string }>({});
  const loadNow = useEffectEvent(load);
  useEffect(() => {
    let current = true;
    loadNow().then(
      (data) => current && setState({ data }),
      (error: unknown) =>
        current &&
        setState({
          problem: `${problemText(error)} Reload the page to try again.`,
        }),
    );
    return () => {
      current = false;
    };
  }, [key]);
  return state;
}
