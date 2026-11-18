-- A personal Pro is paused while its owner holds a seat in an org whose plan includes Pro. The
-- paid time it had left is kept in paid_left; once the last seat ends, resumes_at is when that
-- time runs out and the provider bills again. Only a pause made here carries paused_for_seat_at.
alter table subscriptions
  drop constraint subscriptions_status_check,
  add constraint subscriptions_status_check
    check (status in ('active', 'lapsed', 'paused', 'ended')),
  add column paused_for_seat_at timestamptz,
  add column paid_left interval check (paid_left >= interval '0'),
  add column resumes_at timestamptz,
  add check (paused_for_seat_at is not null or (paid_left is null and resumes_at is null)),
  add check (resumes_at is null or paid_left is null);

-- A settle checks one person's seats against their own Pro and pauses or resumes it to match.
-- A refund may be for part of the payment.
alter table billing_calls
  drop constraint billing_calls_action_check,
  drop constraint billing_calls_check,
  drop constraint billing_calls_check1,
  add constraint billing_calls_action_check
    check (action in ('cancel_now', 'cancel_at_period_end', 'refund', 'settle_seat_pause')),
  add column user_id text references users (id) on delete cascade,
  add column refund_amount bigint check (refund_amount > 0),
  add check ((action = 'refund') = (provider_payment_id is not null)),
  add check ((action in ('cancel_now', 'cancel_at_period_end')) = (subscription is not null)),
  add check ((action = 'settle_seat_pause') = (user_id is not null)),
  add check (action = 'refund' or refund_amount is null);
