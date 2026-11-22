-- An ended org vault entry is now marked one revision above its last, so a stale write to it is refused.
update org_vault_ended set revision = revision + 1;
