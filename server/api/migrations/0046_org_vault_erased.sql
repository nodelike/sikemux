-- The memberships of accounts deleted while their org had a vault, so an owner's or admin's host
-- takes their first name off the org's entries, which only hosts can open.
create table org_vault_erased (
  org_id uuid not null references org_vaults (org_id) on delete cascade,
  membership uuid not null,
  primary key (org_id, membership)
);
