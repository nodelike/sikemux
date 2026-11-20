-- Whether a host approved the code rather than a session, since only a session's approval waits
-- out a recent sign-out before the host it adds can finish.
alter table host_sign_ins add column approved_by_host boolean not null default false;
