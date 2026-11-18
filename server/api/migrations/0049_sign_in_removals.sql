-- A work address the API took off an account's sign-ins, kept by its domain only until the person has been told.
create table sign_in_removals (
  id uuid primary key default gen_random_uuid(),
  user_id text not null references users (id) on delete cascade,
  address_id text not null check (length(address_id) between 1 and 64),
  domain text not null check (length(domain) between 1 and 253),
  org_name text,
  provider text,
  removed_at timestamptz not null default now(),
  told_at timestamptz,
  unique (user_id, address_id)
);

create index sign_in_removals_untold on sign_in_removals (user_id) where told_at is null;
