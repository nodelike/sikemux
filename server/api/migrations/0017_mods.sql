create table mod_counters (
  user_id text primary key references vaults (user_id) on delete cascade,
  seq bigint not null default 0 check (seq >= 0),
  sealed_bytes bigint not null default 0 check (sealed_bytes >= 0),
  cleared_seq bigint not null default 0 check (cleared_seq >= 0 and cleared_seq <= seq)
);

create table mod_documents (
  user_id text not null references mod_counters (user_id) on delete cascade,
  mod_id text not null check (mod_id ~ '^[a-z0-9][a-z0-9-]{0,63}$'),
  doc_id text not null check (doc_id ~ '^[0-9a-f]{64}$'),
  generation integer not null check (generation >= 1),
  sealed text not null check (sealed ~ '^[A-Za-z0-9_-]+$' and length(sealed) <= 87400),
  removed boolean not null,
  revision integer not null check (revision >= 1),
  seq bigint not null check (seq >= 1),
  updated_by text not null,
  updated_at timestamptz not null default now(),
  primary key (user_id, mod_id, doc_id)
);

create unique index mod_documents_by_seq on mod_documents (user_id, seq);
create index mod_documents_removed_by_age on mod_documents (updated_at) where removed;

create table mod_purges (
  user_id text not null references mod_counters (user_id) on delete cascade,
  seq bigint not null check (seq >= 1),
  mod_id text not null check (mod_id ~ '^[a-z0-9][a-z0-9-]{0,63}$'),
  purged_by text not null,
  purged_at timestamptz not null default now(),
  primary key (user_id, seq)
);

create index mod_purges_by_age on mod_purges (purged_at);

alter table events
  drop constraint events_type_check,
  add constraint events_type_check check (
    type in (
      'device.added', 'device.changed', 'device.revoked', 'account.deleted',
      'stash.added', 'stash.changed', 'stash.removed', 'stash.seen',
      'vault.changed', 'mods.changed'
    )
  );
