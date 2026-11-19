-- A person registers a project with at most one of their orgs. Each share names its person, kept
-- equal to its membership's, so the database holds that rule. Of earlier duplicates, the one
-- tapped last stays; a move to another host changes updated_at, so created_at decides.
alter table org_users add constraint org_users_id_user_id_key unique (id, user_id);

alter table org_shares add column user_id text;

update org_shares
set user_id = org_users.user_id
from org_users
where org_users.id = org_shares.member_id;

delete from org_shares
where (member_id, project) in (
  select member_id, project
  from (
    select
      member_id,
      project,
      row_number() over (
        partition by user_id, project
        order by created_at desc, updated_at desc, member_id
      ) as place
    from org_shares
  ) ranked
  where place > 1
);

alter table org_shares
  alter column user_id set not null,
  add constraint org_shares_member_user_fkey foreign key (member_id, user_id)
    references org_users (id, user_id) on update cascade on delete cascade,
  add constraint org_shares_user_id_project_key unique (user_id, project);
