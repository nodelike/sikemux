-- A sale paid on a checkout the API opened but has no record of, so every later event of it is
-- refused with an error, however old, until someone looks.
create table billing_held_sales (
  provider text not null,
  provider_subscription_id text not null,
  provider_checkout_id text not null,
  created_at timestamptz not null default now(),
  primary key (provider, provider_subscription_id)
);
