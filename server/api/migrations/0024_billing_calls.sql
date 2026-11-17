-- What the API has to ask of the payment provider because of something it recorded, such as
-- ending a second plan. It is written in the same transaction and tried until the provider takes
-- it, so a restart in between loses nothing.
create table billing_calls (
  id bigint generated always as identity primary key,
  provider text not null,
  action text not null check (action in ('cancel_now', 'cancel_at_period_end', 'refund')),
  -- The subscription a cancel is for, as the provider is handed it.
  subscription jsonb,
  provider_payment_id text,
  attempts integer not null default 0,
  retry_at timestamptz not null default now(),
  created_at timestamptz not null default now(),
  check ((action = 'refund') = (provider_payment_id is not null)),
  check ((action = 'refund') = (subscription is null))
);

create index billing_calls_due on billing_calls (retry_at);
