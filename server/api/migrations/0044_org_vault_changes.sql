-- Each change to an org's vault takes the next number of that org's changes, so a host asks only
-- for the entries changed or removed since the last number it read.
alter table org_vaults add column changes bigint not null default 0;
alter table org_vault_entries add column change bigint not null default 0;
alter table org_vault_ended add column change bigint not null default 0;

create function number_org_vault_change() returns trigger language plpgsql as $$
begin
  update org_vaults set changes = changes + 1 where org_id = new.org_id
  returning changes into new.change;
  return new;
end;
$$;

create trigger number_org_vault_entry
  before insert or update on org_vault_entries
  for each row execute function number_org_vault_change();

create trigger number_org_vault_ended
  before insert or update on org_vault_ended
  for each row execute function number_org_vault_change();
