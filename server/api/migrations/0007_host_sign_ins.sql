create table host_sign_ins (
  code_hash text primary key check (code_hash ~ '^[0-9a-f]{64}$'),
  user_code text not null unique,
  key text not null check (key ~ '^[0-9a-f]{64}$'),
  name text not null check (length(name) between 1 and 64),
  platform text not null,
  channel text not null,
  approved_by text,
  created_at timestamptz not null default now(),
  expires_at timestamptz not null
);

create index host_sign_ins_by_expiry on host_sign_ins (expires_at);
