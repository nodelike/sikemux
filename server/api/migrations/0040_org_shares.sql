-- A project a member shares with an org, found by its owner's teammates. The project is an opaque
-- hash of its repository's address, and the host is the one that holds the project's record.
create table org_shares (
  member_id uuid not null references org_users (id) on delete cascade,
  project text not null check (project ~ '^[0-9a-f]{64}$'),
  host text not null check (host ~ '^[0-9a-f]{64}$'),
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  primary key (member_id, project)
);

alter table events
  drop constraint events_type_check,
  add constraint events_type_check check (
    type in (
      'device.added', 'device.changed', 'device.revoked', 'account.deleted',
      'stash.added', 'stash.changed', 'stash.removed', 'stash.seen',
      'vault.changed', 'mods.changed', 'plan.changed', 'orgs.changed',
      'shares.changed'
    )
  );
