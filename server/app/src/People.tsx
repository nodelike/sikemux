import type { Org, TeamAccess } from "@sikemux/protocol";
import type { MouseEvent } from "react";

export function PeoplePage({
  org,
}: {
  org: Org;
  team: TeamAccess;
  version: number;
  follow: (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
  reload: () => Promise<void>;
  onLeft: () => Promise<void>;
}) {
  return <p>{org.name}</p>;
}
