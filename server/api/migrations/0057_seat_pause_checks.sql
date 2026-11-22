-- seat_pause_asked_for is the paused_for_seat_at of the last pause asked of the provider, so a
-- retry knows a paused subscription as its own pause whatever time the provider gives it.
alter table subscriptions
  add column seat_pause_asked_for timestamptz;

-- cancel_asked_at is when the person's cancel at the period's end was taken. It stands, whatever the
-- provider holds, until they take it back, and a renewal for a period begun after it is refunded.
alter table subscriptions
  add column cancel_asked_at timestamptz;
update subscriptions set cancel_asked_at = now()
  where cancel_at_period_end and status <> 'ended';

-- kept_without_refund_at is when a cancelled Pro paused for a seat gave up waiting for its current
-- period's payment: it keeps its paid time, as when the refund of it is refused.
alter table subscriptions
  add column kept_without_refund_at timestamptz;

-- resume_asked_at is when we last asked the provider to resume a Pro paused for a seat. The first
-- charge after it starts a new period, whatever the provider calls it.
alter table subscriptions
  add column resume_asked_at timestamptz;

-- When staff are next told that the refund of a payment's kept paid time still has no answer from
-- the provider.
alter table invoices
  add column kept_refund_check_at timestamptz,
  add check (kept_refund_check_at is null or kept_refund is not null);

-- Each refund the provider made of a payment, so the refunds of one payment can be added up.
create table invoice_refunds (
  provider_refund_id text primary key,
  invoice_id uuid not null references invoices (id) on delete cascade,
  amount bigint not null check (amount >= 0)
);
create index invoice_refunds_by_invoice on invoice_refunds (invoice_id);

-- keep_cancel puts the person's cancel back at the provider, and is dropped if by its turn the
-- cancel was taken back, the plan ended or a seat pause is under way.
alter table billing_calls
  drop constraint billing_calls_action_check,
  drop constraint billing_calls_check1,
  add constraint billing_calls_action_check
    check (action in ('cancel_now', 'cancel_at_period_end', 'keep_cancel', 'refund', 'settle_seat_pause')),
  add check ((action in ('cancel_now', 'cancel_at_period_end', 'keep_cancel')) = (subscription is not null));
