import type { Org, OrgPolicy, TeamAccess } from "@sikemux/protocol";
import { IconClock, IconPencil, IconUsers } from "@shared/ui/Icons";
import { useState } from "react";

import { problemText } from "./api.ts";
import { useClient } from "./Billing.tsx";
import { iconMark } from "./marks.ts";
import {
  changeable,
  frozen,
  renewable,
  RULE_WORDS,
  runs,
  useLoad,
} from "./orgs.ts";
import { orgScene } from "./scenes.ts";
import {
  Btn,
  Section,
  Field,
  Page,
  Problem,
  Row,
  Rows,
  Seg,
  Switch,
  Tile,
  useAction,
  useToast,
} from "./ui.tsx";

const DAILY_QUESTIONS = { min: 1, max: 500 };

const RULES = (["every_member", "admins_approve"] as const).map((value) => ({
  value,
  name: RULE_WORDS[value],
}));

export function PolicyPage({ org, team }: { org: Org; team: TeamAccess }) {
  const client = useClient();
  const say = useToast();
  const action = useAction();
  const [policy, setPolicy] = useState<OrgPolicy>();
  const [typed, setTyped] = useState<string>();
  const [unsaved, setUnsaved] = useState<{ next: OrgPolicy; said: string }>();
  const loaded = useLoad(
    async () => (await client("see the org's policy")).getOrgPolicy(org.id),
    org.role,
  );
  const shown = policy ?? loaded.data;
  const editable = runs(org) && changeable(org, team);
  const scene = orgScene(org.id);
  const mark = iconMark("shield");

  if (loaded.problem)
    return (
      <Page
        title="Policy didn't load"
        lede="Nothing in the org has changed."
        actions={<Btn onClick={loaded.again}>Try again</Btn>}
        mark={mark}
        scene={scene}
        tone="failed"
      >
        <Section label="Rules">
          <Problem>{loaded.problem}</Problem>
        </Section>
      </Page>
    );

  const change = (next: OrgPolicy, said: string) => {
    const before = shown;
    setPolicy(next);
    setUnsaved(undefined);
    void action.run(async () => {
      try {
        setPolicy(
          await (
            await client("change the org's policy")
          ).setOrgPolicy(org.id, next),
        );
      } catch (error) {
        setPolicy(before);
        setUnsaved({ next, said });
        throw error;
      } finally {
        setTyped(undefined);
      }
      say(said);
    }, problemText);
  };

  const saveQuestions = () => {
    if (!shown || typed === undefined) return;
    const count = Number(typed);
    if (count === shown.dailyQuestions) return setTyped(undefined);
    if (
      !Number.isInteger(count) ||
      count < DAILY_QUESTIONS.min ||
      count > DAILY_QUESTIONS.max
    ) {
      setTyped(undefined);
      return say(
        `Questions a day must be a whole number from ${DAILY_QUESTIONS.min} to ${DAILY_QUESTIONS.max}.`,
      );
    }
    change(
      { ...shown, dailyQuestions: count },
      `Questions a day per person: ${count}`,
    );
  };

  return (
    <Page
      title={unsaved ? "That wasn't saved" : "Policy"}
      lede={
        unsaved
          ? "The policy is as it was before. Try the change again."
          : runs(org)
            ? `What agents may do across ${org.name}.`
            : "Set by its owners and admins. You can read it."
      }
      actions={
        unsaved ? (
          <>
            <Btn
              busy={action.busy}
              onClick={() => change(unsaved.next, unsaved.said)}
            >
              Save again
            </Btn>
            <Problem>{action.problem}</Problem>
          </>
        ) : runs(org) && !editable ? (
          <p className="note">
            {renewable(org, team)
              ? "The org is frozen. The policy comes back when an owner renews Team."
              : `${frozen(org) ? `${org.name} is paused. ` : ""}Team isn't open to your account now. You can still read the policy.`}
          </p>
        ) : undefined
      }
      mark={mark}
      scene={scene}
      tone={unsaved ? "failed" : undefined}
    >
      <Section label="Rules" busy={!shown}>
        {shown ? (
          <Rows>
            <Row
              wrap
              lead={
                <Tile>
                  <IconPencil size={14} />
                </Tile>
              }
              name="Who can add a rule"
              detail="Rules every agent in the org follows."
              end={
                editable ? (
                  <Seg
                    label="Who can add a rule"
                    value={shown.ruleAdding}
                    disabled={action.busy}
                    onChange={(ruleAdding) => {
                      if (ruleAdding !== shown.ruleAdding)
                        change(
                          { ...shown, ruleAdding },
                          `Who can add a rule: ${RULE_WORDS[ruleAdding].toLowerCase()}`,
                        );
                    }}
                    options={RULES}
                  />
                ) : (
                  RULE_WORDS[shown.ruleAdding]
                )
              }
            />
            <Row
              wrap
              lead={
                <Tile>
                  <IconUsers size={14} />
                </Tile>
              }
              name="Teammates' agents can ask yours"
              detail="About a task they both work on."
              end={
                editable ? (
                  <Switch
                    label="Teammates' agents can ask yours"
                    on={shown.asking}
                    disabled={action.busy}
                    onChange={(asking) =>
                      change(
                        { ...shown, asking },
                        `Teammates' agents asking: ${asking ? "on" : "off"}`,
                      )
                    }
                  />
                ) : shown.asking ? (
                  "On"
                ) : (
                  "Off"
                )
              }
            />
            <Row
              wrap
              lead={
                <Tile>
                  <IconClock size={14} />
                </Tile>
              }
              name="Questions a day, per person"
              detail={`How many one person's agents may ask, across all of ${org.name}'s shared projects.`}
              end={
                editable ? (
                  <Field
                    inputMode="numeric"
                    aria-label="Questions a day, per person"
                    value={typed ?? shown.dailyQuestions}
                    disabled={action.busy}
                    onChange={(event) => setTyped(event.target.value)}
                    onBlur={saveQuestions}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") saveQuestions();
                      if (event.key === "Escape") setTyped(undefined);
                    }}
                  />
                ) : (
                  shown.dailyQuestions
                )
              }
            />
          </Rows>
        ) : null}
      </Section>
    </Page>
  );
}
