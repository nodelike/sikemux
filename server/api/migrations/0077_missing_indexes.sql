create index invoices_by_subscription on invoices (provider_subscription_id);
create index org_shares_by_host on org_shares (host);
create index billing_checkouts_by_opener on billing_checkouts (opened_by);
create index billing_checkouts_by_age on billing_checkouts (created_at);
create index org_vault_entries_by_change on org_vault_entries (org_id, change);
create index org_vault_ended_by_change on org_vault_ended (org_id, change);
create index org_vault_vouches_by_voucher on org_vault_vouches (org_id, by_key);
