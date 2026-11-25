alter table org_invites
  add column email text check (
    email is null
    or (
      email = lower(email)
      and length(email) between 3 and 320
      and position('@' in email) > 1
    )
  ),
  add column sent_at timestamptz;
