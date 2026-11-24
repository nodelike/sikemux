import type { MouseEvent } from "react";

export function JoinPage(_: {
  follow: (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
  onJoined: (orgId: string) => Promise<void>;
}) {
  return null;
}
