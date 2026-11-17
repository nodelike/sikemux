-- When the owner's cut in seats was claimed. An event Paddle sends well after it shows whether
-- Paddle holds the cut, so a claim it never took clears by itself.
alter table subscriptions add column seats_cut_at timestamptz;
update subscriptions set seats_cut_at = now() where seats_cut_to is not null;
alter table subscriptions
  add check (num_nulls(seats_cut_to, seats_cut_at) in (0, 2));
