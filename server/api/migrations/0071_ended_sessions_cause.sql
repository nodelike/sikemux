-- Why the API ended a session: the strip ended it for coming through a work address, or a device
-- was removed and then registered again before Clerk had ended the session it held. Only the
-- strip's refuse a Mac signed in around them; a removed device's hold every sign-in a code's life.
alter table ended_sessions
  add column cause text not null default 'work_address'
    check (cause in ('work_address', 'device_removed'));
