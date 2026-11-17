-- How an org's agents work together. An org that never changed them reads as these defaults.
alter table orgs
  add column rule_adding text not null default 'every_member' check (rule_adding in ('every_member', 'admins_approve')),
  add column asking boolean not null default true,
  add column daily_questions integer not null default 30 check (daily_questions between 1 and 500);
