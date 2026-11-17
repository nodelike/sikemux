-- A checkout erased with the person or org that opened it stays known as one the API opened, so
-- a payment made on it later is ended and refunded rather than taken for a sale gone missing.
create function remember_erased_checkout() returns trigger language plpgsql as $$
begin
  insert into billing_strays (provider, provider_id)
  values (old.provider, old.provider_checkout_id)
  on conflict do nothing;
  return old;
end;
$$;

create trigger remember_erased_checkout
  after delete on billing_checkouts
  for each row execute function remember_erased_checkout();
