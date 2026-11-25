import type { Org, OrgMember, OrgTeam, TeamAccess } from "@sikemux/protocol";
import {
  IconArrowLeft,
  IconClose,
  IconHourglass,
  IconMore,
  IconPause,
  IconPlus,
} from "@shared/ui/Icons";
import { useEffect, useId, useState, type MouseEvent } from "react";

import { problemText } from "./api.ts";
import { useClient } from "./Billing.tsx";
import { iconMark } from "./marks.ts";
import "./members.css";
import { orgPath } from "./navigation.ts";
import {
  changeable,
  frozen,
  renewable,
  ROLE_WORDS,
  runs,
  useLoad,
} from "./orgs.ts";
import { orgScene } from "./scenes.ts";
import { MemberAvatar, nameOf, people } from "./Seats.tsx";
import {
  Btn,
  btnClass,
  Confirm,
  Dialog,
  Nothing,
  Notice,
  Page,
  Popover,
  Problem,
  Search,
  Switch,
  Txt,
  useAction,
  useToast,
} from "./ui.tsx";

function usePhone(): boolean {
  const [phone, setPhone] = useState(
    () =>
      typeof matchMedia !== "undefined" &&
      matchMedia("(max-width: 860px)").matches,
  );
  useEffect(() => {
    const query = matchMedia("(max-width: 860px)");
    const update = () => setPhone(query.matches);
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  }, []);
  return phone;
}

export function GroupsPage({
  org,
  team,
  follow,
}: {
  org: Org;
  team: TeamAccess;
  follow: (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
}) {
  const client = useClient();
  const say = useToast();
  const phone = usePhone();
  const [query, setQuery] = useState("");
  const [selectedId, setSelectedId] = useState<string>();
  const [phoneList, setPhoneList] = useState(true);
  const [dialog, setDialog] = useState<
    "new" | "people" | "rename" | "delete"
  >();
  const [removingId, setRemovingId] = useState<string>();
  const [inlineProblem, setInlineProblem] = useState<string>();

  const loaded = useLoad(async () => {
    const routes = await client("see your groups");
    const [{ teams }, { members }] = await Promise.all([
      routes.listOrgTeams(org.id),
      routes.listOrgMembers(org.id),
    ]);
    return { teams, members };
  }, org.role);

  const changed = (text: string, undo?: () => Promise<void>) => {
    setInlineProblem(undefined);
    say(text, undo);
    loaded.again();
  };

  const scene = orgScene(org.id);
  const mark = iconMark("tag");
  const open = changeable(org, team);
  const ended = renewable(org, team);
  const editable = runs(org) && open;
  const billing = orgPath(org.id, "billing");

  if (loaded.problem)
    return (
      <Page
        title="Groups"
        lede="Short lists of people you name."
        mark={mark}
        scene={scene}
        tone="failed"
      >
        <Nothing
          title="Groups didn't load"
          actions={<Btn onClick={loaded.again}>Try again</Btn>}
        >
          {loaded.problem} Nothing in the org has changed.
        </Nothing>
      </Page>
    );

  const { teams, members } = loaded.data ?? {};
  const boss = members?.find((each) => each.role === "owner");
  const bossName = boss?.name ?? "An owner";

  const notice = frozen(org) ? (
    ended ? (
      <Notice
        icon={<IconHourglass size={17} />}
        tone="cold"
        title={`${org.name} is frozen`}
        actions={
          org.role === "owner" ? (
            <a
              className={btnClass({})}
              href={billing}
              onClick={follow(billing)}
            >
              Renew Team
            </a>
          ) : (
            <span className="m-dim">{bossName} can renew it</span>
          )
        }
      >
        Its Team plan ended. Nothing is deleted.
      </Notice>
    ) : (
      <Notice icon={<IconPause size={17} />} title={`${org.name} is paused`}>
        Team isn't open to your account now. Nothing is deleted.
      </Notice>
    )
  ) : !open && teams ? (
    <Notice
      icon={<IconPause size={17} />}
      title="Team isn't open to your account now"
    >
      You can still read groups.
    </Notice>
  ) : null;

  const trimmed = query.trim().toLowerCase();
  const filteredTeams = (teams ?? []).filter((each) =>
    !trimmed ? true : each.name.toLowerCase().includes(trimmed),
  );
  const selected =
    filteredTeams.find((each) => each.id === selectedId) ?? filteredTeams[0];
  const inside = selected
    ? (members ?? []).filter((member) => selected.members.includes(member.id))
    : [];

  const showTools =
    teams !== undefined && teams.length > 0 && !(phone && !phoneList);

  const tools = showTools ? (
    <>
      <Search label="Find a group" value={query} onChange={setQuery} />
      <span className="gap" />
      {editable ? (
        <Btn onClick={() => setDialog("new")}>
          <IconPlus size={14} />
          Make a group
        </Btn>
      ) : null}
    </>
  ) : undefined;

  const lede =
    teams && teams.length > 0
      ? `${teams.length} ${teams.length === 1 ? "group" : "groups"}. Short lists of people you name.`
      : "Short lists of people you name.";

  const removeMember = async (group: OrgTeam, member: OrgMember) => {
    const name = nameOf(member);
    setRemovingId(member.id);
    setInlineProblem(undefined);
    try {
      const routes = await client("change a group");
      await routes.removeOrgTeamMember(org.id, group.id, member.id);
      changed(`${name} is out of ${group.name}`, async () => {
        await (
          await client("change a group")
        ).addOrgTeamMember(org.id, group.id, member.id);
        changed(`${name} is back in ${group.name}`);
      });
    } catch (error) {
      setInlineProblem(`${problemText(error)} The group is unchanged.`);
    } finally {
      setRemovingId(undefined);
    }
  };

  return (
    <Page
      title="Groups"
      lede={lede}
      notice={notice}
      tools={tools}
      mark={mark}
      scene={scene}
      tone={frozen(org) ? "waiting" : undefined}
    >
      {!teams || !members ? (
        <GroupsSkeleton />
      ) : teams.length === 0 ? (
        <Nothing
          title="No groups yet"
          actions={
            editable ? (
              <Btn onClick={() => setDialog("new")}>
                <IconPlus size={14} />
                Make a group
              </Btn>
            ) : undefined
          }
        >
          A group is a short list of people you name, so agents can ask the
          right ones.
        </Nothing>
      ) : filteredTeams.length === 0 ? (
        <Nothing
          title="No groups match"
          actions={
            <Btn quiet onClick={() => setQuery("")}>
              Clear
            </Btn>
          }
        >
          Try another group name.
        </Nothing>
      ) : (
        <div className={phoneList ? "m-gr list" : "m-gr"}>
          <nav aria-label="Groups">
            {filteredTeams.map((group) => {
              const groupMembers = members.filter((member) =>
                group.members.includes(member.id),
              );
              return (
                <button
                  type="button"
                  key={group.id}
                  aria-current={
                    !phone && selected?.id === group.id ? "true" : undefined
                  }
                  onClick={() => {
                    setSelectedId(group.id);
                    setPhoneList(false);
                    setInlineProblem(undefined);
                  }}
                >
                  <b>{group.name}</b>
                  <span className="stack" aria-hidden="true">
                    {groupMembers.slice(0, 4).map((member) => (
                      <MemberAvatar key={member.id} member={member} size={22} />
                    ))}
                  </span>
                  <em>{groupMembers.length}</em>
                </button>
              );
            })}
          </nav>

          {selected ? (
            <div className="m-gb">
              <div className="m-gh">
                {phone ? (
                  <Txt onClick={() => setPhoneList(true)}>
                    <IconArrowLeft size={14} />
                    Groups
                  </Txt>
                ) : null}
                <h2>{selected.name}</h2>
                <span>{people(inside.length)}</span>
                <i className="gap" />
                {editable ? (
                  <>
                    <Btn small quiet onClick={() => setDialog("people")}>
                      <IconPlus size={14} />
                      {phone ? "Add" : "Add people"}
                    </Btn>
                    <Popover
                      look="icon"
                      label={`More for ${selected.name}`}
                      trigger={<IconMore size={16} />}
                      options={[
                        {
                          key: "rename",
                          name: "Rename",
                          choose: () => setDialog("rename"),
                        },
                        {
                          key: "delete",
                          name: "Delete group",
                          detail: "Its members stay in the org",
                          bad: true,
                          choose: () => setDialog("delete"),
                        },
                      ]}
                    />
                  </>
                ) : null}
              </div>

              {inlineProblem ? <Problem>{inlineProblem}</Problem> : null}

              {inside.length > 0 ? (
                <div className="m-pp">
                  {inside.map((member) => {
                    const name = nameOf(member);
                    return (
                      <div key={member.id}>
                        <MemberAvatar member={member} size={32} />
                        <span>
                          <b>{name}</b>
                          <small>{ROLE_WORDS[member.role]}</small>
                        </span>
                        {editable ? (
                          <button
                            type="button"
                            className="ico"
                            aria-label={`Take ${name} out`}
                            disabled={removingId === member.id}
                            onClick={() => void removeMember(selected, member)}
                          >
                            <IconClose size={14} />
                          </button>
                        ) : null}
                      </div>
                    );
                  })}
                </div>
              ) : (
                <p className="m-say">Nobody is in {selected.name} yet.</p>
              )}
            </div>
          ) : null}
        </div>
      )}

      {dialog === "new" && editable ? (
        <NewGroupDialog
          org={org}
          onCreated={(created) => {
            setSelectedId(created.id);
            setPhoneList(false);
            changed(`Made ${created.name}`);
            setDialog("people");
          }}
          onClose={() => setDialog(undefined)}
        />
      ) : null}

      {dialog === "people" && selected && members && editable ? (
        <GroupPeopleDialog
          org={org}
          group={selected}
          members={members}
          onChanged={changed}
          onClose={() => setDialog(undefined)}
        />
      ) : null}

      {dialog === "rename" && selected && editable ? (
        <RenameGroupDialog
          org={org}
          group={selected}
          onRenamed={(text) => {
            setDialog(undefined);
            changed(text);
          }}
          onClose={() => setDialog(undefined)}
        />
      ) : null}

      {dialog === "delete" && selected && editable ? (
        <DeleteGroupDialog
          org={org}
          group={selected}
          onDeleted={(text) => {
            setDialog(undefined);
            setSelectedId(undefined);
            setPhoneList(true);
            changed(text);
          }}
          onClose={() => setDialog(undefined)}
        />
      ) : null}
    </Page>
  );
}

function GroupsSkeleton() {
  return (
    <div className="m-sklist" aria-busy="true">
      {Array.from({ length: 5 }, (_, at) => (
        <div key={at}>
          <span className="skel m-skel-av" />
          <span>
            <span
              className="skel m-skel-bar"
              style={{ width: 140 + ((at * 47) % 90) }}
            />
            <span
              className="skel m-skel-bar"
              style={{ width: 200 + ((at * 31) % 80) }}
            />
          </span>
        </div>
      ))}
    </div>
  );
}

function NewGroupDialog({
  org,
  onCreated,
  onClose,
}: {
  org: Org;
  onCreated: (created: OrgTeam) => void;
  onClose: () => void;
}) {
  const client = useClient();
  const action = useAction();
  const inputId = useId();
  const [name, setName] = useState("");

  const submit = () => {
    const wanted = name.trim();
    if (!wanted) return;
    void action.run(
      async () => {
        const created = await (
          await client("make a group")
        ).createOrgTeam(org.id, { name: wanted });
        onCreated(created);
      },
      (error) => `${problemText(error)} No group was made.`,
    );
  };

  return (
    <Dialog
      title="Make a group"
      busy={action.busy}
      onClose={onClose}
      onSubmit={submit}
      foot={
        <>
          <Btn quiet disabled={action.busy} onClick={onClose}>
            Cancel
          </Btn>
          <Btn type="submit" busy={action.busy} disabled={!name.trim()}>
            Make group
          </Btn>
        </>
      }
    >
      <div className="m-fl">
        <label htmlFor={inputId}>Name</label>
        <input
          id={inputId}
          className="field"
          placeholder="Platform"
          value={name}
          maxLength={64}
          autoFocus
          onChange={(event) => setName(event.target.value)}
        />
      </div>
      <p className="m-hint">You pick its people next.</p>
      <Problem>{action.problem}</Problem>
    </Dialog>
  );
}

function GroupPeopleDialog({
  org,
  group,
  members,
  onChanged,
  onClose,
}: {
  org: Org;
  group: OrgTeam;
  members: OrgMember[];
  onChanged: (text: string) => void;
  onClose: () => void;
}) {
  const client = useClient();
  const [query, setQuery] = useState("");
  const [busyId, setBusyId] = useState<string>();
  const [problem, setProblem] = useState<string>();
  const trimmed = query.trim().toLowerCase();
  const shown = trimmed
    ? members.filter((each) => nameOf(each).toLowerCase().includes(trimmed))
    : members;

  const toggle = async (member: OrgMember, on: boolean) => {
    const name = nameOf(member);
    setBusyId(member.id);
    setProblem(undefined);
    try {
      const routes = await client("change a group");
      if (on) await routes.addOrgTeamMember(org.id, group.id, member.id);
      else await routes.removeOrgTeamMember(org.id, group.id, member.id);
      onChanged(
        on ? `${name} is in ${group.name}` : `${name} is out of ${group.name}`,
      );
    } catch (error) {
      setProblem(`${problemText(error)} The group is unchanged.`);
    } finally {
      setBusyId(undefined);
    }
  };

  return (
    <Dialog
      title={`Add to ${group.name}`}
      busy={busyId !== undefined}
      onClose={onClose}
      foot={<Btn onClick={onClose}>Done</Btn>}
    >
      <div className="m-dlg-search">
        <Search label="Find a person" value={query} onChange={setQuery} />
      </div>
      <Problem>{problem}</Problem>
      {shown.length === 0 ? (
        <p className="m-hint">Nobody matches "{query}".</p>
      ) : (
        <div className="m-picklist">
          {shown.map((member) => {
            const name = nameOf(member);
            const inside = group.members.includes(member.id);
            return (
              <div key={member.id} className="m-pickrow">
                <MemberAvatar member={member} size={26} />
                <span className="m-pickrow-who">
                  <b>{name}</b>
                  <small>{ROLE_WORDS[member.role]}</small>
                </span>
                <Switch
                  label={`${name} in ${group.name}`}
                  on={inside}
                  disabled={busyId === member.id}
                  onChange={(on) => void toggle(member, on)}
                />
              </div>
            );
          })}
        </div>
      )}
    </Dialog>
  );
}

function RenameGroupDialog({
  org,
  group,
  onRenamed,
  onClose,
}: {
  org: Org;
  group: OrgTeam;
  onRenamed: (text: string) => void;
  onClose: () => void;
}) {
  const client = useClient();
  const action = useAction();
  const inputId = useId();
  const [name, setName] = useState(group.name);

  return (
    <Confirm
      title={`Rename ${group.name}`}
      busy={action.busy}
      problem={action.problem}
      label="Rename"
      unchanged={!name.trim() || name.trim() === group.name}
      onCancel={onClose}
      onConfirm={() =>
        void action.run(
          async () => {
            const renamed = await (
              await client("rename a group")
            ).renameOrgTeam(org.id, group.id, { name: name.trim() });
            onRenamed(`${group.name} is now ${renamed.name}`);
          },
          (error) => `${problemText(error)} Its name is unchanged.`,
        )
      }
    >
      <div className="m-fl">
        <label htmlFor={inputId}>Name</label>
        <input
          id={inputId}
          className="field"
          value={name}
          maxLength={64}
          autoFocus
          onChange={(event) => setName(event.target.value)}
        />
      </div>
    </Confirm>
  );
}

function DeleteGroupDialog({
  org,
  group,
  onDeleted,
  onClose,
}: {
  org: Org;
  group: OrgTeam;
  onDeleted: (text: string) => void;
  onClose: () => void;
}) {
  const client = useClient();
  const action = useAction();
  return (
    <Confirm
      danger
      title={`Delete ${group.name}?`}
      busy={action.busy}
      problem={action.problem}
      label="Delete group"
      onCancel={onClose}
      onConfirm={() =>
        void action.run(
          async () => {
            await (
              await client("delete a group")
            ).deleteOrgTeam(org.id, group.id);
            onDeleted(`Deleted ${group.name}`);
          },
          (error) => `${problemText(error)} The group is still there.`,
        )
      }
    >
      <p>Its members stay in {org.name}; only the list goes.</p>
    </Confirm>
  );
}
