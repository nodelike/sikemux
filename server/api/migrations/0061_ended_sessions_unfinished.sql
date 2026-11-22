-- Empty until Clerk has ended the session. No Mac may add itself while any of the account's is.
alter table ended_sessions alter column ended_at drop not null;
alter table ended_sessions alter column ended_at drop default;
