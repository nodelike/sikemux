import type { Org, OrgOffer, TeamAccess } from "@sikemux/protocol";
import type { MouseEvent } from "react";

/** Starting an org of one's own, or joining one that takes the person in. */
export function StartPage(_: {
  team: TeamAccess;
  staff: boolean;
  orgs: Org[];
  offers: OrgOffer[];
  follow: (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
  onJoined: (orgId: string) => Promise<void>;
}) {
  return null;
}
