-- A client that drives the account's hosts but never signs for the person there.
alter table devices add column hands_only boolean not null default false;
