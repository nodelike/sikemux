-- Removing a host signs out every other sign-in of the account, so none can sign that Mac in again. A
-- row waits here until Clerk has ended them. kept_session is the remover's own, left signed in.
create table host_removal_sign_outs (
  id uuid primary key default gen_random_uuid(),
  user_id text not null references users (id) on delete cascade,
  kept_session text,
  removed_at timestamptz not null default now(),
  clerk_attempts integer not null default 0,
  clerk_retry_at timestamptz not null default now(),
  done_at timestamptz
);

create index host_removal_sign_outs_due on host_removal_sign_outs (clerk_retry_at) where done_at is null;
