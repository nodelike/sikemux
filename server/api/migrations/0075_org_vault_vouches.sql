-- by_key has no reference: what a host signed outlives it, for hosts that have yet to read it.
create table org_vault_vouches (
  org_id uuid not null references org_vaults (org_id) on delete cascade,
  host_key text not null references devices (key) on delete cascade,
  member uuid not null,
  by_key text not null,
  by_member uuid not null,
  at bigint not null check (at > 0),
  signature text not null check (signature ~ '^[A-Za-z0-9_-]{86}$'),
  introduction text check (introduction ~ '^[A-Za-z0-9_-]+$' and length(introduction) <= 180000),
  primary key (org_id, host_key, by_key),
  check (host_key <> by_key)
);

create index org_vault_vouches_by_host on org_vault_vouches (host_key);
