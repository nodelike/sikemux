-- A strip that could not read an account at Clerk may have recorded no removal to wait on. The account
-- is run again on a timer instead, backing off.
alter table users
  add column strip_attempts integer not null default 0,
  add column strip_retry_at timestamptz;

create index users_awaiting_strip on users (strip_retry_at) where strip_retry_at is not null;
