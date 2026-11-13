alter table mod_counters
  add column file_bytes bigint not null default 0 check (file_bytes >= 0);

create table mod_files (
  user_id text not null references mod_counters (user_id) on delete cascade,
  mod_id text not null check (mod_id ~ '^[a-z0-9][a-z0-9-]{0,63}$'),
  file_id text not null check (file_id ~ '^[0-9a-f]{64}$'),
  stored_as text not null unique check (stored_as ~ '^[A-Za-z0-9_-]{22}$'),
  size bigint not null check (size >= 0),
  stored_by text not null,
  stored_at timestamptz not null default now(),
  primary key (user_id, mod_id, file_id)
);
