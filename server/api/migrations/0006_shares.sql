create table shares (
  id text primary key,
  user_id text not null references users (id) on delete cascade,
  host_key text not null,
  created_at timestamptz not null default now(),
  expires_at timestamptz not null,
  ended_at timestamptz,
  ended_reason text check (ended_reason in ('stopped', 'expired')),
  check ((ended_at is null) = (ended_reason is null))
);

create index shares_open_by_user on shares (user_id) where ended_at is null;
create index shares_by_host on shares (host_key) where ended_at is null;
