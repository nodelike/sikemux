-- When the person last tapped Share for a registration. A move to another host or a rename keeps
-- it, so when a rename meets a registration already under the new name, the later tap stays.
alter table org_shares add column tapped_at timestamptz not null default now();
update org_shares set tapped_at = created_at;
