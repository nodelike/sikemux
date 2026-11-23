-- A page a host shared: its sealed HTML lives in the stash's file store under stored_as until the
-- share ends or expires. The row stays a while after, so the host can still read how it ended.
create table pages (
  id text primary key check (id ~ '^[A-Za-z0-9_-]{22}$'),
  user_id text not null references users (id) on delete cascade,
  stored_as text unique check (stored_as ~ '^[A-Za-z0-9_-]{22}$'),
  size bigint not null check (size >= 0),
  viewer_check text not null check (viewer_check ~ '^[0-9a-f]{64}$'),
  anyone boolean not null,
  views integer not null default 0,
  expires_at timestamptz not null,
  ended_at timestamptz,
  stored_by text not null,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index pages_live on pages (user_id) where ended_at is null;
create index pages_expiry on pages (expires_at);
