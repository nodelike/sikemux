import type { Org } from "@sikemux/protocol";

export function DomainsPage({ org }: { org: Org }) {
  return <p>{org.name}</p>;
}
