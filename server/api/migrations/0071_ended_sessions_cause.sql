-- Why the API ended a session: the strip ended it for coming through a work address, or a device
-- was removed and then registered again before Clerk had ended the session it held. Only the
-- strip's refuse a Mac signed in around them; a removed device's hold every sign-in a code's life.
alter table ended_sessions
  add column cause text not null default 'work_address'
    check (cause in ('work_address', 'device_removed'));

-- Rows recorded before the cause was: registering a removed device again wrote its session's row in
-- one transaction with its audit entry, so both carry the same moment. The device may have come back
-- on another account than its session's, so the entry's account is not compared.
update ended_sessions e
set cause = 'device_removed'
where exists (
  select 1 from audit a
  where a.action = 'device.registered'
    and a.at = e.made_at
);
