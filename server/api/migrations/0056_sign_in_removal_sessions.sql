-- Each stretch of time an address was on an account, from the earliest Clerk read or webhook that
-- showed it until the strip took it away or found it gone. Clerk's own time starts again when an
-- address is removed and added back, so a stretch only ever starts earlier, and a sighting older
-- than a stretch's end belongs to that stretch. The domain is kept, keyed like the address, so a
-- work address deleted before the strip saw it is still known by its org; could_sign_in is set
-- once any sighting showed it verified, reserved or brought by a connected account.
create table addresses_seen (
  id bigint generated always as identity primary key,
  user_id text not null references users (id) on delete cascade,
  address_hash bytea not null,
  domain_hash bytea not null,
  first_seen timestamptz not null,
  ended_at timestamptz,
  could_sign_in boolean not null default false
);

create unique index addresses_seen_open on addresses_seen (user_id, address_hash) where ended_at is null;
create index addresses_seen_ended on addresses_seen (user_id, address_hash, ended_at) where ended_at is not null;

-- The sessions a removed work address could have opened are those made since its stretch began.
-- end_sessions_from holds that time until they have been ended, then goes back to null and
-- swept_from keeps it, so a stretch found to start earlier is swept again.
alter table sign_in_removals add column end_sessions_from timestamptz;
alter table sign_in_removals add column swept_from timestamptz;
alter table sign_in_removals add column address_seen_id bigint references addresses_seen (id) on delete cascade;

create index sign_in_removals_sessions_left on sign_in_removals (user_id) where end_sessions_from is not null;

-- Set when a session through a work address may have taken the account over. While it is set,
-- no sign-in session or Mac sign-in is taken and no device is added; staff clear it by hand.
alter table users add column frozen_at timestamptz;

-- The Clerk session that put a device on the account: the one that registered it, approved its
-- code, or put on the host that added it. Null for a Mac that added itself with its own sign-in.
alter table devices add column sign_in_session text;
alter table host_sign_ins add column approved_session text;
-- When the Mac's own sign-in that registered the device was made.
alter table devices add column mac_signed_in_at timestamptz;
alter table account_folds add column started_session text;

create index devices_sign_in_session on devices (sign_in_session) where sign_in_session is not null;

-- Sessions the strip ended. Their tokens are refused at once, before they expire, and a Mac
-- signed in between made_at and ended_at may not add itself.
create table ended_sessions (
  session_id text primary key,
  user_id text not null references users (id) on delete cascade,
  made_at timestamptz not null,
  ended_at timestamptz not null default now()
);

create index ended_sessions_user on ended_sessions (user_id);
