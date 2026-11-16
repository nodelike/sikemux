-- A membership's own id, so the org's pages act on a member without learning their sign-in's id.
alter table org_users add column id uuid not null default gen_random_uuid();
alter table org_users add constraint org_users_id_key unique (id);
