-- Who pays for an org's plans: the owner who bought them. When they stop being an owner it is
-- cleared, their plans are set to end, and nobody else acts on their payment method.
alter table billing_customers add column paid_by text references users (id) on delete set null;
alter table billing_customers add check (org_id is not null or paid_by is null);

update billing_customers customer
set paid_by = (
  select user_id from org_users
  where org_id = customer.org_id and role = 'owner'
  order by joined_at, user_id
  limit 1
)
where org_id is not null;
