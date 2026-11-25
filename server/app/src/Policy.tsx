import type { Org, OrgPolicy, TeamAccess } from "@sikemux/protocol";
import {
  IconCircleAlert,
  IconHourglass,
  IconMinus,
  IconPause,
  IconPlus,
  IconRefresh,
} from "@shared/ui/Icons";
import { useState } from "react";

import { problemText } from "./api.ts";
import { useClient } from "./client.ts";
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
import "./settings.css";
import {
  Btn,
  Nothing,
  Notice,
  Page,
  Section,
  Seg,
  Setting,
  Switch,
  useAction,
  useToast,
} from "./ui.tsx";

const DAILY_QUESTIONS = { min: 1, max: 500, step: 5 };

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
  const lede = `What agents may do across ${org.name}.`;

  if (loaded.problem)
    return (
      <Page title="Rules" lede={lede} mark={mark} scene={scene} tone="failed">
        <Nothing
          title="Rules didn't load"
          actions={
            <Btn quiet onClick={loaded.again}>
              <IconRefresh size={14} />
              Try again
            </Btn>
          }
        >
          {loaded.problem} Nothing in the org has changed.
        </Nothing>
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

  const stepQuestions = (delta: number) => {
    if (!shown) return;
    const count = Math.min(
      DAILY_QUESTIONS.max,
      Math.max(DAILY_QUESTIONS.min, shown.dailyQuestions + delta),
    );
    if (count === shown.dailyQuestions) return;
    change(
      { ...shown, dailyQuestions: count },
      `Questions a day per person: ${count}`,
    );
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

  const notice = unsaved ? (
    <Notice
      icon={<IconCircleAlert size={16} />}
      tone="bad"
      title="That wasn't saved"
      actions={
        <Btn
          quiet
          busy={action.busy}
          onClick={() => change(unsaved.next, unsaved.said)}
        >
          Save again
        </Btn>
      }
    >
      {action.problem ??
        "The rules are as they were before. Try the change again."}
    </Notice>
  ) : runs(org) && !editable ? (
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
        renewable(org, team) ? `${org.name} is frozen` : `${org.name} is paused`
      }
    >
      {renewable(org, team)
        ? "Its Team plan ended. The rules come back when an owner renews Team."
        : `${frozen(org) ? "Nothing is deleted. " : ""}Team isn't open to your account now. You can still read the rules.`}
    </Notice>
  ) : undefined;

  return (
    <Page
      title="Rules"
      lede={lede}
      notice={notice}
      mark={mark}
      scene={scene}
      tone={unsaved ? "failed" : undefined}
    >
      {!shown ? (
        <Section busy />
      ) : (
        <>
          {!runs(org) ? (
            <p className="note">
              Set by its owners and admins. You can read it.
            </p>
          ) : null}
          <Setting
            name="Who can add a rule"
            detail="Rules every agent in the org follows."
          >
            {editable ? (
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
              <span className="set-val">{RULE_WORDS[shown.ruleAdding]}</span>
            )}
            <p className="note">
              {shown.ruleAdding === "every_member"
                ? "Anyone's new rule counts at once."
                : "A member's new rule waits for an owner or admin."}
            </p>
          </Setting>

          <Setting
            name="Teammates' agents can ask yours"
            detail="About a task they both work on."
          >
            {editable ? (
              <div className="set-line">
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
                <span className="set-switch-label">
                  {shown.asking ? "Allowed" : "Off for everyone"}
                </span>
              </div>
            ) : (
              <span className="set-val">{shown.asking ? "Yes" : "No"}</span>
            )}
            <p className="note">
              {shown.asking
                ? "A question goes to your agent before it goes to you."
                : "Their agents ask you instead."}
            </p>
          </Setting>

          <Setting
            name="Questions a day, per person"
            detail={`How many one person's agents may ask, across all of ${org.name}'s shared projects.`}
          >
            {editable ? (
              <span className={shown.asking ? "set-step" : "set-step set-dim"}>
                <button
                  type="button"
                  aria-label="Fewer questions a day"
                  disabled={
                    action.busy || shown.dailyQuestions <= DAILY_QUESTIONS.min
                  }
                  onClick={() => stepQuestions(-DAILY_QUESTIONS.step)}
                >
                  <IconMinus size={14} />
                </button>
                <input
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
                <button
                  type="button"
                  aria-label="More questions a day"
                  disabled={
                    action.busy || shown.dailyQuestions >= DAILY_QUESTIONS.max
                  }
                  onClick={() => stepQuestions(DAILY_QUESTIONS.step)}
                >
                  <IconPlus size={14} />
                </button>
              </span>
            ) : (
              <span className="set-val">{shown.dailyQuestions} a day</span>
            )}
            <p className="note">
              {!shown.asking
                ? "Not used while agents can't ask each other."
                : `From ${DAILY_QUESTIONS.min} to ${DAILY_QUESTIONS.max} a day for each person's agents.`}
            </p>
          </Setting>
        </>
      )}
    </Page>
  );
}
