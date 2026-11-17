create table orgs (
  id uuid primary key default gen_random_uuid(),
  name text not null check (length(name) between 1 and 64),
  created_by text not null,
  created_at timestamptz not null default now()
);

-- One row is one seat. owner: billing, ownership and deleting the org. admin: people and invites.
create table org_users (
  org_id uuid not null references orgs (id) on delete cascade,
  user_id text not null references users (id) on delete cascade,
  role text not null check (role in ('owner', 'admin', 'user')),
  invited_by text,
  joined_at timestamptz not null default now(),
  primary key (org_id, user_id)
);

create index org_users_by_user on org_users (user_id);

-- A link the inviter copies and sends; it holds a seat until accepted, revoked or expired.
create table org_invites (
  id uuid primary key default gen_random_uuid(),
  org_id uuid not null references orgs (id) on delete cascade,
  role text not null check (role in ('owner', 'admin', 'user')),
  token_hash text not null unique check (token_hash ~ '^[0-9a-f]{64}$'),
  invited_by text not null,
  created_at timestamptz not null default now(),
  expires_at timestamptz not null,
  accepted_by text,
  accepted_at timestamptz,
  revoked_at timestamptz,
  check ((accepted_at is null) = (accepted_by is null))
);

create index org_invites_open on org_invites (org_id, expires_at)
  where accepted_at is null and revoked_at is null;
