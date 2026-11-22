-- A share open to anyone with its link may be watched without signing in.
alter table shares add column anyone boolean not null default false;
