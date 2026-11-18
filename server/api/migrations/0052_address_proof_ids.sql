-- A proof's own id, so removing one never carries its keyed hash in an address.
alter table address_proofs add column id uuid not null default gen_random_uuid() unique;
