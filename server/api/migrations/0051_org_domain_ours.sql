-- When an owner said addresses at this domain are the org's own. Only an address added after it is
-- refused a Sikemux account or sign-in of its own; null means none is.
alter table org_domains
  add column ours_since timestamptz,
  add constraint org_domains_ours_after_verified check (ours_since is null or verified_at is not null);
