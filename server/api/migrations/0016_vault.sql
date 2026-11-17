create table vaults (
  user_id text primary key references users (id) on delete cascade,
  generation integer not null default 1 check (generation >= 1),
  recovery_key text not null check (recovery_key ~ '^[0-9a-f]{64}$'),
  recovery_wrap text not null check (recovery_wrap ~ '^[A-Za-z0-9_-]+$' and length(recovery_wrap) <= 87400),
  rotate boolean not null default false,
  created_by text not null,
  created_at timestamptz not null default now(),
  rotated_at timestamptz
);

create table vault_keys (
  user_id text not null references vaults (user_id) on delete cascade,
  host_key text not null check (host_key ~ '^[0-9a-f]{64}$'),
  generation integer not null check (generation >= 1),
  wrapped text not null check (wrapped ~ '^[A-Za-z0-9_-]+$' and length(wrapped) <= 87400),
  wrapped_by text not null,
  created_at timestamptz not null default now(),
  primary key (user_id, host_key)
);

create table vault_entries (
  id text primary key check (id ~ '^[0-9a-f]{32}$'),
  user_id text not null references vaults (user_id) on delete cascade,
  plugin text not null check (plugin ~ '^sikemux\.[a-z0-9-]{1,48}$'),
  generation integer not null check (generation >= 1),
  sealed text not null check (sealed ~ '^[A-Za-z0-9_-]+$' and length(sealed) <= 87400),
  revision integer not null check (revision >= 1),
  updated_by text not null,
  updated_at timestamptz not null default now(),
  created_at timestamptz not null default now()
);

create index vault_entries_by_user on vault_entries (user_id);

create table vault_uses (
  id bigserial primary key,
  user_id text not null references users (id) on delete cascade,
  entry_id text not null check (entry_id ~ '^[0-9a-f]{32}$'),
  plugin text not null check (plugin ~ '^sikemux\.[a-z0-9-]{1,48}$'),
  host_key text not null,
  actor text not null check (length(actor) between 1 and 80),
  count integer not null check (count between 1 and 1000000),
  first_at timestamptz not null,
  last_at timestamptz not null check (last_at >= first_at)
);

create index vault_uses_by_user on vault_uses (user_id, last_at desc);
create index vault_uses_by_age on vault_uses (last_at);

alter table events
  drop constraint events_type_check,
  add constraint events_type_check check (
    type in (
      'device.added', 'device.changed', 'device.revoked', 'account.deleted',
      'stash.added', 'stash.changed', 'stash.removed', 'stash.seen',
      'vault.changed'
    )
  );
