-- A host's reports of one sign-in by one actor add up into one row for each UTC day.
create index vault_uses_by_day on vault_uses (
  user_id,
  entry_id,
  host_key,
  actor,
  ((first_at at time zone 'UTC')::date)
);
