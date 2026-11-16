-- Paddle keeps one customer per email address, so a person paying for themselves and for an org
-- they own is the same Paddle customer behind two billing customers.
drop index billing_customers_by_provider;
