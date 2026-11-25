import type { Org, TeamAccess } from "@sikemux/protocol";
import { IconOrg } from "@shared/ui/Icons";
import { useState } from "react";

import { problemText } from "./api.ts";
import { orgSvg } from "./avatarSvg.ts";
import { useClient } from "./Billing.tsx";
import { changeable, frozen, renewable, runs } from "./orgs.ts";
import { orgScene } from "./scenes.ts";
import {
  Btn,
  Card,
  Field,
  Page,
  Row,
  Rows,
  Tile,
  useAction,
  useToast,
} from "./ui.tsx";

const NAME_LENGTH = 64;

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
  const say = useToast();
  const action = useAction();
  const [typed, setTyped] = useState<string>();
  const editable = runs(org) && changeable(org, team);
  const name = typed?.trim();
  const changed = name !== undefined && name !== org.name;

  const rename = () => {
    if (!name || !changed) return;
    void action.run(async () => {
      await (await client("rename the org")).renameOrg(org.id, { name });
      await reload();
      setTyped(undefined);
      say(`Now called ${name}`);
    }, problemText);
  };

  const keep = () => {
    setTyped(undefined);
    action.clear();
  };

  return (
    <Page
      title={action.problem ? "That name wasn't saved" : org.name}
      lede={
        action.problem ??
        (runs(org)
          ? "The name everyone in the org sees."
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
      mark={{ svg: orgSvg(org.id) }}
      scene={orgScene(org.id)}
      tone={action.problem ? "failed" : undefined}
    >
      <Card label="Details" w={8}>
        <Rows>
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
                    <Btn type="submit" busy={action.busy} disabled={!name}>
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
      </Card>
    </Page>
  );
}
