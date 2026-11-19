-- A challenge the API issued after trading a Mac's sign-in code at Clerk: only it can add a Mac
-- with no token, and it carries when that sign-in was made. Null on every other challenge.
alter table challenges add column mac_signed_in_at timestamptz;
