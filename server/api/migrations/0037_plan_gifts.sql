-- A plan given to a person free until a date, with no subscription behind it.
create table plan_gifts (
  user_id text primary key references users (id) on delete cascade,
  plan text not null check (plan in ('pro')),
  given_at timestamptz not null default now(),
  ends_at timestamptz not null
);

create index plan_gifts_by_end on plan_gifts (ends_at);
