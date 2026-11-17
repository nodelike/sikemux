-- Each checkout the API opens, under the provider's id for it. A payment is matched to its
-- checkout here, never by anything the buyer could change on the provider's page.
create table billing_checkouts (
  provider text not null,
  provider_checkout_id text not null,
  customer_id uuid not null references billing_customers (id) on delete cascade,
  opened_by text not null references users (id) on delete cascade,
  provider_customer_id text not null,
  product text not null check (product in ('pro', 'team', 'shed', 'shed_always_on', 'shed_large')),
  interval text not null check (interval in ('month', 'year')),
  quantity integer not null check (quantity > 0),
  created_at timestamptz not null default now(),
  primary key (provider, provider_checkout_id)
);
