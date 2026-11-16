-- A cut in an org's seats, now or at renewal, as the owner took it. The org fills no more seats
-- than this from that moment, before the provider's event lowers the quantity.
alter table subscriptions add column seats_cut_to integer check (seats_cut_to > 0);
