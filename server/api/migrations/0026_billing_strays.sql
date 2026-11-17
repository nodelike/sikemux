create table billing_strays (
  provider text not null,
  provider_id text not null,
  created_at timestamptz not null default now(),
  primary key (provider, provider_id)
);
