create table org_vaults (
  org_id uuid primary key references orgs (id) on delete cascade,
  generation integer not null default 1 check (generation >= 1),
  created_by text not null,
  created_at timestamptz not null default now(),
  rotated_at timestamptz
);

create table org_vault_keys (
  org_id uuid not null references org_vaults (org_id) on delete cascade,
  host_key text not null check (host_key ~ '^[0-9a-f]{64}$'),
  generation integer not null check (generation >= 1),
  wrapped text not null check (wrapped ~ '^[A-Za-z0-9_-]+$' and length(wrapped) <= 256),
  wrapped_by text not null,
  created_at timestamptz not null default now(),
  primary key (org_id, host_key)
);

create table org_vault_entries (
  id text primary key check (id ~ '^[0-9a-f]{32}$'),
  org_id uuid not null references org_vaults (org_id) on delete cascade,
  kind text not null check (kind ~ '^[a-z][a-z0-9-]{0,47}$'),
  generation integer not null check (generation >= 1),
  sealed text not null check (sealed ~ '^[A-Za-z0-9_-]+$' and length(sealed) <= 87400),
  revision integer not null check (revision >= 1),
  updated_by text not null,
  updated_at timestamptz not null default now(),
  created_at timestamptz not null default now()
);

create index org_vault_entries_by_org on org_vault_entries (org_id);
