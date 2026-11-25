import type { Org, TeamAccess } from "@sikemux/protocol";
import { useAuth } from "@clerk/react";
import { IconHourglass, IconPause, IconUpload } from "@shared/ui/Icons";
import { useRef, useState, type DragEvent } from "react";

import { problemText } from "./api.ts";
import { OrgAvatar } from "./Avatar.tsx";
import { useClient } from "./client.ts";
import { fitLogo, LOGO_TYPES, sendLogo, useLogo } from "./logos.ts";
import { iconMark } from "./marks.ts";
import { changeable, frozen, renewable, runs } from "./orgs.ts";
import { orgScene } from "./scenes.ts";
import "./settings.css";
import {
  Btn,
  Field,
  Notice,
  Page,
  Problem,
  Setting,
  Txt,
  useAction,
  useToast,
} from "./ui.tsx";

const NAME_LENGTH = 64;
const LOGO_BYTES = 1_000_000;

export function DetailsPage({
  org,
  team,
  reload,
}: {
  org: Org;
  team: TeamAccess;
  reload: () => Promise<void>;
}) {
  const client = useClient();
  const { getToken } = useAuth();
  const say = useToast();
  const action = useAction();
  const [changing, setChanging] = useState<"name" | "logo">("name");
  const [typed, setTyped] = useState<string>();
  const [over, setOver] = useState(false);
  const chooser = useRef<HTMLInputElement>(null);
  const logo = useLogo(org);
  const editable = runs(org) && changeable(org, team);
  const name = typed?.trim();
  const changed = name !== undefined && name !== org.name;
  const busy = (what: typeof changing) => action.busy && changing === what;

  const rename = () => {
    if (!name || !changed) return;
    setChanging("name");
    void action.run(async () => {
      await (await client("rename the org")).renameOrg(org.id, { name });
      await reload();
      setTyped(undefined);
      say(`Now called ${name}`);
    }, problemText);
  };

  const keep = () => {
    setTyped(undefined);
    if (changing === "name") action.clear();
  };

  const send = async (picture: Blob) => {
    await sendLogo(org.id, picture, getToken);
    await reload();
  };
  const remove = async () => {
    await (await client("remove the org's logo")).removeOrgLogo(org.id);
    await reload();
  };
  const was = logo?.state === "ready" ? logo.picture : undefined;

  const choose = (file: File | undefined) => {
    if (!file || action.busy) return;
    setChanging("logo");
    void action.run(async () => {
      if (!LOGO_TYPES.includes(file.type))
        throw new Error("It has to be a PNG, JPG or WebP.");
      if (file.size > LOGO_BYTES)
        throw new Error(`It's ${sizeOf(file.size)}. Choose one up to 1 MB.`);
      await send(await fitLogo(file));
      say(
        org.logo ? "Logo replaced" : "Logo added",
        was ? () => send(was) : org.logo ? undefined : remove,
      );
    }, problemText);
  };

  const takeAway = () => {
    setChanging("logo");
    void action.run(async () => {
      await remove();
      say("Logo removed", was && (() => send(was)));
    }, problemText);
  };

  const onDragOver = (event: DragEvent) => {
    if (!event.dataTransfer.types.includes("Files")) return;
    event.preventDefault();
    setOver(true);
  };
  const onDragLeave = (event: DragEvent) => {
    if (!event.currentTarget.contains(event.relatedTarget as Node | null))
      setOver(false);
  };
  const onDrop = (event: DragEvent) => {
    event.preventDefault();
    setOver(false);
    choose(event.dataTransfer.files[0]);
  };

  const notice =
    runs(org) && !editable ? (
      <Notice
        icon={
          renewable(org, team) ? (
            <IconHourglass size={16} />
          ) : (
            <IconPause size={16} />
          )
        }
        tone={renewable(org, team) ? "cold" : undefined}
        title={
          renewable(org, team)
            ? `${org.name} is frozen`
            : `${org.name} is paused`
        }
      >
        {renewable(org, team)
          ? "Its Team plan ended. Its details can change again when an owner renews Team."
          : `${frozen(org) ? "Nothing is deleted. " : ""}Team isn't open to your account now. You can still read its details.`}
      </Notice>
    ) : undefined;

  return (
    <Page
      title="Settings"
      lede={runs(org) ? "Its name and logo." : "Set by its owners and admins."}
      notice={notice}
      mark={iconMark("org")}
      scene={orgScene(org.id)}
      tone={action.problem ? "failed" : undefined}
    >
      <Setting name="Name" detail="On invites, shared projects and bills.">
        {editable ? (
          <form
            className="set-name"
            onSubmit={(event) => {
              event.preventDefault();
              rename();
            }}
          >
            <Field
              aria-label="Org name"
              value={typed ?? org.name}
              maxLength={NAME_LENGTH}
              autoComplete="off"
              disabled={action.busy}
              onChange={(event) => setTyped(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Escape") keep();
              }}
            />
            <Btn
              type="submit"
              quiet={!changed}
              busy={busy("name")}
              disabled={!name || !changed || action.busy}
            >
              Save
            </Btn>
            {changed ? (
              <Btn quiet disabled={action.busy} onClick={keep}>
                Cancel
              </Btn>
            ) : null}
          </form>
        ) : (
          <span className="set-val">{org.name}</span>
        )}
        {changing === "name" ? <Problem>{action.problem}</Problem> : null}
      </Setting>

      <Setting
        name={over ? "Drop it to use it" : "Logo"}
        detail={
          editable
            ? "PNG, JPG or WebP, up to 1 MB. Choose one or drop it here."
            : "Set by its owners and admins."
        }
      >
        <div
          className={over ? "set-drop over" : "set-drop"}
          {...(editable ? { onDragOver, onDragLeave, onDrop } : {})}
        >
          <span className="set-logo">
            <OrgAvatar org={org} size={72} />
            {busy("logo") ? (
              <span className="set-logo-wait" aria-hidden="true" />
            ) : null}
          </span>
          <div className="set-acts">
            {editable ? (
              <>
                <input
                  ref={chooser}
                  type="file"
                  accept={LOGO_TYPES.join(",")}
                  hidden
                  onChange={(event) => {
                    choose(event.target.files?.[0]);
                    event.target.value = "";
                  }}
                />
                <Btn
                  quiet
                  busy={busy("logo")}
                  disabled={action.busy}
                  onClick={() => chooser.current?.click()}
                >
                  <IconUpload size={14} />
                  {busy("logo") ? "Uploading" : org.logo ? "Replace" : "Upload"}
                </Btn>
                {org.logo ? (
                  <Txt disabled={action.busy} onClick={takeAway}>
                    Remove
                  </Txt>
                ) : null}
              </>
            ) : !org.logo ? (
              <span className="note">None yet.</span>
            ) : null}
          </div>
        </div>
        {changing === "logo" ? <Problem>{action.problem}</Problem> : null}
      </Setting>
    </Page>
  );
}

function sizeOf(bytes: number): string {
  const size = bytes / 1_000_000;
  return `${size < 10 ? (Math.ceil(size * 10) / 10).toString() : Math.round(size)} MB`;
}
