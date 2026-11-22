-- The last revision of each entry removed from an account's vault. A removed entry's id never
-- comes back: the server refuses a write to it, and hosts take no entry under it.
create table vault_ended (
  user_id text not null references vaults (user_id) on delete cascade,
  id text not null check (id ~ '^[0-9a-f]{32}$'),
  revision integer not null check (revision >= 1),
  primary key (user_id, id)
);
