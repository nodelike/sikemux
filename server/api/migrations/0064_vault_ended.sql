-- The last revision of each entry removed from an account's vault. Hosts hold a removed entry
-- to it, so the server cannot hand one back, and an entry added under its id starts above it.
create table vault_ended (
  user_id text not null references vaults (user_id) on delete cascade,
  id text not null check (id ~ '^[0-9a-f]{32}$'),
  revision integer not null check (revision >= 1),
  primary key (user_id, id)
);
