-- A payment the payer's bank took back is disputed, and the plan it paid for ends.
alter table invoices drop constraint invoices_status_check;
alter table invoices add constraint invoices_status_check
  check (status in ('paid', 'failed', 'refunded', 'disputed'));
