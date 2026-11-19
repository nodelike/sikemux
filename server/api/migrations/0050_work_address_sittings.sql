-- One go at proving a work address through Google, from the account that started it. The proof it
-- makes counts toward joining an org only through this row, once, soon after.
create table work_address_sittings (
  id uuid primary key default gen_random_uuid(),
  user_id text not null references users (id) on delete cascade,
  state_hash bytea not null unique check (length(state_hash) = 32),
  nonce text not null,
  verifier text not null,
  address_hash bytea check (length(address_hash) = 32),
  created_at timestamptz not null default now(),
  proven_at timestamptz,
  joined_at timestamptz
);

create index work_address_sittings_by_user on work_address_sittings (user_id);
