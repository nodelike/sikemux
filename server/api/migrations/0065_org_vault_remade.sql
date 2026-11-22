-- When an owner's or admin's host last made the org's vault again, null if none has.
alter table org_vaults add column remade_at timestamptz;
