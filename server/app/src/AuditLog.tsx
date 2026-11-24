import type { Org } from "@sikemux/protocol";

export function AuditLogPage({ org }: { org: Org }) {
  return <p>{org.name}</p>;
}
