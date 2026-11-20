-- resume_asked_for is the time we asked the provider to resume a Pro paused for a seat at. Only a
-- charge whose period starts between resume_asked_at and about a day after it counts as the resume.
alter table subscriptions
  add column resume_asked_for timestamptz;

-- When the provider made each refund, so refunds made after a point can be added up.
alter table invoice_refunds
  add column at timestamptz not null default now();
alter table invoice_refunds
  alter column at drop default;
