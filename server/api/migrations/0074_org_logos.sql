-- The picture an org shows its members. Its id changes with the picture, so whoever keeps a copy
-- can tell it is an old one.
create table org_logos (
  org_id uuid primary key references orgs (id) on delete cascade,
  id uuid not null unique default gen_random_uuid(),
  mime text not null check (mime in ('image/png', 'image/jpeg', 'image/webp')),
  bytes bytea not null check (length(bytes) between 1 and 1000000)
);
