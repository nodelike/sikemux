-- A work address one account has proven, kept as a keyed hash: the address itself is never stored.
create table address_proofs (
  address_hash bytea primary key check (length(address_hash) = 32),
  user_id text not null references users (id) on delete cascade,
  domain text not null check (length(domain) between 1 and 253),
  org_id uuid references orgs (id) on delete set null,
  via text not null check (via in ('fold', 'google')),
  proven_at timestamptz not null default now()
);

create index address_proofs_by_user on address_proofs (user_id);
create index address_proofs_by_domain on address_proofs (domain);

-- A wish to fold one account into another. One side's live session makes it; the other side's confirms it.
create table account_folds (
  id uuid primary key default gen_random_uuid(),
  from_user text references users (id) on delete cascade,
  into_user text references users (id) on delete cascade,
  made_session text not null,
  secret_hash bytea not null check (length(secret_hash) = 32),
  why text not null check (why in ('invite', 'share', 'browser', 'domain', 'address', 'seat')),
  org_id uuid references orgs (id) on delete set null,
  address_hash bytea check (length(address_hash) = 32),
  domain text,
  created_at timestamptz not null default now(),
  expires_at timestamptz not null,
  done_at timestamptz,
  check (from_user is not null or into_user is not null),
  check (done_at is null or (from_user is not null and into_user is not null and address_hash is not null))
);

create index account_folds_by_from on account_folds (from_user) where done_at is not null;

alter table users add column admitted_at timestamptz;
alter table users add column new_kept_at timestamptz;
update users set admitted_at = created_at;
