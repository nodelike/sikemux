-- Who may use Team, whatever is sold: one switch, staff alone until someone opens it. The vaults
-- of orgs have their own, as what people put in one is their real keys.
create table team_audience (
  only_row boolean primary key default true check (only_row),
  audience text not null default 'staff' check (audience in ('staff', 'nightly', 'everyone')),
  vaults_open boolean not null default false
);
insert into team_audience default values;

-- The seats an org has free, until it has a Team plan of its own.
alter table orgs add column preview_seats integer check (preview_seats > 0);
update orgs set preview_seats = 20;

-- The releases a host last said it follows, and when it said so.
alter table devices
  add column followed_channel text check (followed_channel in ('stable', 'nightly', 'staff')),
  add column followed_at timestamptz;
