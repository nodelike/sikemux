create table host_tokens (
  token_hash text primary key check (token_hash ~ '^[0-9a-f]{64}$'),
  user_id text not null,
  created_at timestamptz not null default now(),
  expires_at timestamptz not null
);

create index host_tokens_by_expiry on host_tokens (expires_at);
