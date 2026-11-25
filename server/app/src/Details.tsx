import type { Org, TeamAccess } from "@sikemux/protocol";
import { useAuth } from "@clerk/react";
import { IconOrg } from "@shared/ui/Icons";
import { useRef, useState, type DragEvent } from "react";

import { problemText } from "./api.ts";
import { OrgAvatar } from "./Avatar.tsx";
import { useClient } from "./Billing.tsx";
import { fitLogo, LOGO_TYPES, sendLogo, useLogo, useOrgMark } from "./logos.ts";
import { changeable, frozen, renewable, runs } from "./orgs.ts";
import { orgScene } from "./scenes.ts";
import {
  Btn,
  Section,
  Field,
  Page,
  Row,
  Rows,
  Tile,
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
  const mark = useOrgMark(org);
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

  return (
    <Page
      title={
        action.problem
          ? changing === "name"
            ? "That name wasn't saved"
            : "That logo wasn't saved"
          : org.name
      }
      lede={
        action.problem ??
        (runs(org)
          ? "The name and logo everyone in the org sees."
          : "Set by its owners and admins.")
      }
      actions={
        runs(org) && !editable ? (
          <p className="note">
            {renewable(org, team)
              ? "The org is frozen. Its details can change again when an owner renews Team."
              : `${frozen(org) ? `${org.name} is paused. ` : ""}Team isn't open to your account now. You can still read its details.`}
          </p>
        ) : undefined
      }
      mark={mark}
      scene={orgScene(org.id)}
      tone={action.problem ? "failed" : undefined}
    >
      <Section
        label="Details"
        className={over ? "details over" : "details"}
        {...(editable ? { onDragOver, onDragLeave, onDrop } : {})}
      >
        <Rows>
          <Row
            wrap
            lead={<OrgAvatar org={org} size={40} />}
            name="Logo"
            detail={
              editable ? (
                <>
                  PNG, JPG or WebP, up to 1 MB.
                  <span className="drops"> Choose one or drop it here.</span>
                </>
              ) : org.logo ? (
                "Everyone in the org sees it."
              ) : (
                "None yet."
              )
            }
            end={
              editable ? (
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
                  {org.logo ? (
                    <Txt disabled={action.busy} onClick={takeAway}>
                      Remove
                    </Txt>
                  ) : null}
                  <Btn
                    quiet={!!org.logo}
                    busy={busy("logo")}
                    disabled={action.busy}
                    onClick={() => chooser.current?.click()}
                  >
                    {org.logo ? "Replace" : "Upload"}
                  </Btn>
                </>
              ) : undefined
            }
          />
          <Row
            wrap
            lead={
              <Tile>
                <IconOrg size={14} />
              </Tile>
            }
            name="Name"
            detail="On invites, shared projects and bills."
            end={
              editable ? (
                <form
                  className="named"
                  onSubmit={(event) => {
                    event.preventDefault();
                    rename();
                  }}
                >
                  <Field
                    aria-label="Name"
                    value={typed ?? org.name}
                    maxLength={NAME_LENGTH}
                    autoComplete="off"
                    disabled={action.busy}
                    onChange={(event) => setTyped(event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key === "Escape") keep();
                    }}
                  />
                  {changed ? (
                    <Btn
                      type="submit"
                      busy={busy("name")}
                      disabled={!name || action.busy}
                    >
                      Save
                    </Btn>
                  ) : null}
                </form>
              ) : (
                org.name
              )
            }
          />
        </Rows>
      </Section>
    </Page>
  );
}

function sizeOf(bytes: number): string {
  const size = bytes / 1_000_000;
  return `${size < 10 ? (Math.ceil(size * 10) / 10).toString() : Math.round(size)} MB`;
}
