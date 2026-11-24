import type { Org } from "@sikemux/protocol";

export function PolicyPage({ org }: { org: Org }) {
  return <p>{org.name}</p>;
}
