-- A named list of an org's members. It owns nothing: it stands in wherever a list of people goes.
create table org_teams (
  id uuid primary key default gen_random_uuid(),
  org_id uuid not null references orgs (id) on delete cascade,
  name text not null check (length(name) between 1 and 64),
  created_at timestamptz not null default now(),
  unique (org_id, name)
);

create table org_team_members (
  team_id uuid not null references org_teams (id) on delete cascade,
  member_id uuid not null references org_users (id) on delete cascade,
  added_at timestamptz not null default now(),
  primary key (team_id, member_id)
);

create index org_team_members_by_member on org_team_members (member_id);

-- A domain an org proves it owns with a DNS TXT record. Nobody's address is kept: only the domain.
create table org_domains (
  id uuid primary key default gen_random_uuid(),
  org_id uuid not null references orgs (id) on delete cascade,
  domain text not null check (
    length(domain) <= 253
    and domain ~ '^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$'
  ),
  token text not null check (token ~ '^[A-Za-z0-9_-]{22}$'),
  joining text not null default 'offered' check (joining in ('open', 'offered')),
  added_by text not null,
  created_at timestamptz not null default now(),
  verified_at timestamptz,
  unique (org_id, domain)
);

create unique index org_domains_verified on org_domains (domain) where verified_at is not null;

-- The org's audit log reads its rows by what they were about, newest first.
create index audit_by_subject on audit (subject, id) where subject is not null;
