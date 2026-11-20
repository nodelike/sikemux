-- A removal still waiting on its sessions is run again on a timer, oldest due first, backing off
-- while it keeps failing, so a few that never clear can't hold the rest back. A minute's grace
-- leaves the first try to the strip that recorded it.
alter table sign_in_removals
  add column rerun_attempts integer not null default 0,
  add column rerun_at timestamptz not null default now() + interval '1 minute';

create index sign_in_removals_rerun_due on sign_in_removals (rerun_at) where end_sessions_from is not null;
