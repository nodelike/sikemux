-- Each person's own customer at a provider, made at their first checkout and used for every one
-- after. One provider customer belongs to one person.
create table billing_people (
  provider text not null,
  user_id text not null references users (id) on delete cascade,
  provider_customer_id text not null,
  created_at timestamptz not null default now(),
  primary key (provider, user_id),
  unique (provider, provider_customer_id)
);
