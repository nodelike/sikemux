-- A deleted account's subscriptions are ended with the payment provider by the API's sweeper:
-- at the period's end when the account is deleted, at once when it is due to be erased. A failed
-- call is tried again later, backing off.
alter table users
  add column billing_attempts integer not null default 0,
  add column billing_retry_at timestamptz;

create index users_awaiting_billing on users (billing_retry_at) where billing_retry_at is not null;
