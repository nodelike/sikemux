-- Entries and ended rows written before changes were numbered all read 0, so a full read that asks
-- for changes past 0 missed them. Touching each row gives it the next number of its org's changes.
update org_vault_entries set change = change where change = 0;
update org_vault_ended set change = change where change = 0;
