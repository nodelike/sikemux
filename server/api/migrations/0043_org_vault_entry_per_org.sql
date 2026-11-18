-- An entry's id is unique within its org only, so two orgs sharing a repository never meet.
alter table org_vault_entries
  drop constraint org_vault_entries_pkey,
  add primary key (org_id, id);

drop index org_vault_entries_by_org;

-- The last revision of each entry removed from an org's vault. The next entry under the same id
-- starts above it, so a host that held the old one takes the new.
create table org_vault_ended (
  org_id uuid not null references org_vaults (org_id) on delete cascade,
  id text not null check (id ~ '^[0-9a-f]{32}$'),
  revision integer not null check (revision >= 1),
  primary key (org_id, id)
);
