-- Who pays: one person or one org, as the payment provider knows them. The provider keeps
-- every card and address; its customer id arrives with its first event.
create table billing_customers (
  id uuid primary key default gen_random_uuid(),
  user_id text references users (id) on delete cascade,
  org_id uuid references orgs (id) on delete cascade,
  provider text not null check (provider ~ '^[a-z][a-z_-]*$'),
  provider_customer_id text,
  created_at timestamptz not null default now(),
  check ((user_id is null) <> (org_id is null))
);

create unique index billing_customers_by_user on billing_customers (user_id, provider) where user_id is not null;
create unique index billing_customers_by_org on billing_customers (org_id, provider) where org_id is not null;
create unique index billing_customers_by_provider on billing_customers (provider, provider_customer_id)
  where provider_customer_id is not null;

-- One product bought in a quantity. A customer holds a plan and any number of Shed products
-- side by side. Some providers give each product its own subscription; others put several
-- products under one, so a provider's subscription may be several rows here, one per product.
create table subscriptions (
  id uuid primary key default gen_random_uuid(),
  customer_id uuid not null references billing_customers (id) on delete cascade,
  provider_subscription_id text not null,
  product text not null check (product in ('pro', 'team', 'shed', 'shed_always_on', 'shed_large')),
  interval text not null check (interval in ('month', 'year')),
  quantity integer not null check (quantity > 0),
  status text not null check (status in ('active', 'lapsed', 'ended')),
  current_period_end timestamptz not null,
  cancel_at_period_end boolean not null default false,
  started_at timestamptz not null,
  ended_at timestamptz,
  -- A change asked for at renewal, which the provider applies when the period ends.
  next_product text check (next_product in ('pro', 'team', 'shed', 'shed_always_on', 'shed_large')),
  next_interval text check (next_interval in ('month', 'year')),
  next_quantity integer check (next_quantity > 0),
  -- When the provider last said what the subscription is, so an older event arriving late is ignored.
  provider_updated_at timestamptz not null,
  check ((status = 'ended') = (ended_at is not null)),
  check (num_nulls(next_product, next_interval, next_quantity) in (0, 3))
);

create unique index subscriptions_by_provider on subscriptions (provider_subscription_id, product);
create index subscriptions_by_customer on subscriptions (customer_id) where status <> 'ended';
create unique index subscriptions_one_plan on subscriptions (customer_id)
  where status <> 'ended' and product in ('pro', 'team');

-- What the provider billed, kept so receipts and refunds can be answered without asking it.
create table invoices (
  id uuid primary key default gen_random_uuid(),
  customer_id uuid not null references billing_customers (id) on delete cascade,
  provider_subscription_id text not null,
  provider_payment_id text not null,
  -- What was paid for: [{product, quantity}], one for each product under the subscription.
  lines jsonb not null check (jsonb_typeof(lines) = 'array' and jsonb_array_length(lines) > 0),
  reason text not null check (reason in ('first_payment', 'renewal', 'change')),
  amount bigint not null check (amount >= 0),
  currency text not null check (currency ~ '^[a-z]{3}$'),
  status text not null check (status in ('paid', 'failed', 'refunded')),
  at timestamptz not null,
  -- A refund is asked of the provider, which may approve it later; it is refunded only once it says so.
  refund_requested_at timestamptz,
  refunded_at timestamptz,
  check ((status = 'refunded') = (refunded_at is not null))
);

create unique index invoices_by_provider on invoices (provider_payment_id);
create index invoices_by_customer on invoices (customer_id, at desc);

-- Every provider event handled, so one sent again, even after a restart, changes nothing.
-- Only the id and type: the payload's personal and payment details are never kept.
create table billing_events (
  provider text not null,
  event_id text not null,
  type text not null,
  received_at timestamptz not null default now(),
  primary key (provider, event_id)
);

create index billing_events_by_age on billing_events (received_at);

alter table events
  drop constraint events_type_check,
  add constraint events_type_check check (
    type in (
      'device.added', 'device.changed', 'device.revoked', 'account.deleted',
      'stash.added', 'stash.changed', 'stash.removed', 'stash.seen',
      'vault.changed', 'mods.changed', 'plan.changed'
    )
  );
