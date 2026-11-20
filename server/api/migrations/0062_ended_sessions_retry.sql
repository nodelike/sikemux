-- A session the strip could not get Clerk to end is tried again on a timer, by its own id, since
-- Clerk may stop listing it before the strip next runs. A minute's grace leaves the first try to
-- the strip that recorded it.
alter table ended_sessions
  add column clerk_attempts integer not null default 0,
  add column clerk_retry_at timestamptz not null default now() + interval '1 minute';

create index ended_sessions_unfinished on ended_sessions (clerk_retry_at) where ended_at is null;
