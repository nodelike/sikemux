import type {
  Org,
  OrgDomain,
  OrgDomainJoining,
  TeamAccess,
} from "@sikemux/protocol";
import { formatDay } from "@shared/lib/format";
import { IconGlobe } from "@shared/ui/Icons";
import { useState } from "react";

import { problemText } from "./api.ts";
import { useClient } from "./Billing.tsx";
import { useCheer } from "./Field.tsx";
import { iconMark } from "./marks.ts";
import { changeable, frozen, possessive, useLoad } from "./orgs.ts";
import { orgScene } from "./scenes.ts";
import {
  Btn,
  Card,
  Chip,
  Column,
  Confirm,
  Empty,
  Field,
  Page,
  Problem,
  Row,
  Rows,
  Seg,
  Switch,
  Tile,
  Txt,
  useAction,
  useToast,
} from "./ui.tsx";

export const JOINING_WORDS: Record<OrgDomainJoining, string> = {
  open: "Join on their own",
  offered: "Offered at sign-in",
};

const JOINING = (["open", "offered"] as const).map((value) => ({
  value,
  name: JOINING_WORDS[value],
}));

const either = (words: string[]) =>
  words.length < 2
    ? words.join("")
    : `${words.slice(0, -1).join(", ")} or ${words[words.length - 1]}`;

export function DomainsPage({ org, team }: { org: Org; team: TeamAccess }) {
  const client = useClient();
  const say = useToast();
  const [adding, setAdding] = useState(false);
  const [missing, setMissing] = useState<{
    domain: OrgDomain;
    words: string;
  }>();
  const loaded = useLoad(
    async () => (await client("see your domains")).listOrgDomains(org.id),
    org.role,
  );
  const open = changeable(org, team);
  const scene = orgScene(org.id);
  const mark = iconMark("at");
  const domains = loaded.data?.domains;
  const verified = domains?.filter((each) => each.verifiedAt) ?? [];
  const waiting = domains?.filter((each) => !each.verifiedAt) ?? [];
  const changed = (text: string) => {
    say(text);
    setMissing(undefined);
    loaded.again();
  };

  if (loaded.problem)
    return (
      <Page
        title="Domains didn't load"
        lede="Nothing in the org has changed."
        actions={<Btn onClick={loaded.again}>Try again</Btn>}
        mark={mark}
        scene={scene}
        tone="failed"
      >
        <Card label="Domains">
          <Problem>{loaded.problem}</Problem>
        </Card>
      </Page>
    );

  const form =
    adding && open ? (
      <NewDomain
        org={org}
        onAdded={(text) => {
          setAdding(false);
          changed(text);
        }}
        onCancel={() => setAdding(false)}
      />
    ) : null;
  const add =
    form ??
    (domains && open ? (
      <Btn onClick={() => setAdding(true)}>Add a domain</Btn>
    ) : domains ? (
      <p className="note">
        {frozen(org)
          ? "The org is frozen. You can still remove a domain."
          : "Team isn't open to your account now. You can still remove a domain."}
      </p>
    ) : undefined);

  if (domains?.length === 0)
    return (
      <Page
        title="Let colleagues in"
        lede="Prove your domain and people with an address there find the org by themselves."
        actions={add}
        mark={mark}
        scene={scene}
      >
        <Card label="Domains">
          <Empty>No domains yet. Invite links work without one.</Empty>
        </Card>
      </Page>
    );

  const joins = verified.filter((each) => each.joining === "open");
  return (
    <Page
      title={missing ? "No record yet" : "Domains"}
      lede={
        missing
          ? missing.words
          : !domains
            ? "Loading the domains…"
            : joins.length > 0
              ? `People with a ${either(joins.map((each) => each.domain))} address join by themselves.`
              : verified.length > 0
                ? `People with a ${either(verified.map((each) => each.domain))} address are offered ${org.name} when they sign in.`
                : "Nobody can join through a domain until its record is found."
      }
      actions={add}
      mark={mark}
      scene={scene}
      tone={missing ? "failed" : undefined}
    >
      {!domains ? (
        <Card label="Domains" w={7} busy />
      ) : (
        <>
          {verified.length > 0 ? (
            <Card
              label="Verified"
              count={verified.length}
              w={waiting.length > 0 ? 7 : 8}
            >
              <Rows>
                {verified.map((domain) => (
                  <Verified
                    key={domain.id}
                    org={org}
                    domain={domain}
                    open={open}
                    onChanged={changed}
                  />
                ))}
              </Rows>
            </Card>
          ) : null}
          {waiting.length > 0 ? (
            <Column w={verified.length > 0 ? 5 : 8} side={verified.length > 0}>
              <Card
                label="Waiting for a record"
                count={waiting.length}
                foot="Add this TXT record where your domain lives."
              >
                <Rows>
                  {waiting.map((domain) => (
                    <Waiting
                      key={domain.id}
                      org={org}
                      domain={domain}
                      open={open}
                      missing={missing?.domain.id === domain.id}
                      onChanged={changed}
                      onMissing={(words) =>
                        setMissing(words ? { domain, words } : undefined)
                      }
                    />
                  ))}
                </Rows>
              </Card>
            </Column>
          ) : null}
        </>
      )}
    </Page>
  );
}

function NewDomain({
  org,
  onAdded,
  onCancel,
}: {
  org: Org;
  onAdded: (text: string) => void;
  onCancel: () => void;
}) {
  const client = useClient();
  const action = useAction();
  const [domain, setDomain] = useState("");
  const [joining, setJoining] = useState<OrgDomainJoining>("offered");
  return (
    <>
      <form
        className="form"
        onSubmit={(event) => {
          event.preventDefault();
          if (!domain.trim()) return;
          void action.run(
            async () => {
              const added = await (
                await client("add a domain")
              ).addOrgDomain(org.id, { domain: domain.trim(), joining });
              onAdded(`Added ${added.domain}. Put its TXT record in DNS next.`);
            },
            (error) => `${problemText(error)} No domain was added.`,
          );
        }}
      >
        <Field
          placeholder="example.com"
          aria-label="Email domain"
          value={domain}
          maxLength={253}
          autoFocus
          onChange={(event) => setDomain(event.target.value)}
        />
        <Seg
          label="How people join"
          value={joining}
          onChange={setJoining}
          options={JOINING}
        />
        <Btn type="submit" busy={action.busy} disabled={!domain.trim()}>
          Add
        </Btn>
        <Btn quiet disabled={action.busy} onClick={onCancel}>
          Cancel
        </Btn>
      </form>
      <Problem>{action.problem}</Problem>
    </>
  );
}

const lead = (
  <Tile>
    <IconGlobe size={14} />
  </Tile>
);

function Verified({
  org,
  domain,
  open,
  onChanged,
}: {
  org: Org;
  domain: OrgDomain;
  open: boolean;
  onChanged: (text: string) => void;
}) {
  const client = useClient();
  const action = useAction();
  const [removing, setRemoving] = useState(false);

  const setJoining = (joining: OrgDomainJoining) =>
    void action.run(
      async () => {
        await (
          await client("change how people join")
        ).setOrgDomainJoining(org.id, domain.id, { joining });
        onChanged(`${domain.domain}: ${JOINING_WORDS[joining].toLowerCase()}`);
      },
      (error) => `${problemText(error)} It is unchanged.`,
    );
  const setOurs = (ours: boolean) =>
    void action.run(
      async () => {
        await (
          await client("change whether the domain's addresses are yours")
        ).setOrgDomainOurs(org.id, domain.id, { ours });
        onChanged(
          `Addresses at ${domain.domain} ${ours ? "are" : "aren't"} ${possessive(org.name)} own`,
        );
      },
      (error) => `${problemText(error)} It is unchanged.`,
    );

  return (
    <Row
      lead={lead}
      name={
        <>
          {domain.domain}
          <Chip tone="ok">Verified</Chip>
        </>
      }
      detail={`Since ${formatDay(domain.verifiedAt ?? domain.createdAt, "short")}`}
      end={
        removing ? null : (
          <Txt
            bad
            onClick={() => {
              action.clear();
              setRemoving(true);
            }}
          >
            Remove
          </Txt>
        )
      }
    >
      {removing ? (
        <RemoveDomain
          org={org}
          domain={domain}
          onChanged={onChanged}
          onCancel={() => setRemoving(false)}
        />
      ) : (
        <div className="box">
          <Rows>
            <Row
              wrap
              name="People with an address"
              detail={
                domain.joining === "open"
                  ? "They join on a free seat, as Member."
                  : "They are asked when they sign in, and join as Member."
              }
              end={
                open ? (
                  <Seg
                    label="How people join"
                    value={domain.joining}
                    disabled={action.busy}
                    onChange={(joining) => {
                      if (joining !== domain.joining) setJoining(joining);
                    }}
                    options={JOINING}
                  />
                ) : (
                  JOINING_WORDS[domain.joining]
                )
              }
            />
            <Row
              wrap
              name="These addresses are ours"
              detail={
                domain.oursSince
                  ? `Since ${formatDay(domain.oursSince, "short")}, an address at ${domain.domain} can't sign up for its own Sikemux account or be added as a sign-in. People verify it under Work addresses instead. Accounts and sign-ins from before then stay.`
                  : `Turn this on to stop new Sikemux accounts and sign-ins at ${domain.domain}. People then sign in with their own account and verify their ${domain.domain} address under Work addresses. Accounts and sign-ins that already exist stay.`
              }
              end={
                open && org.role === "owner" ? (
                  <Switch
                    label="These addresses are ours"
                    on={domain.oursSince !== null}
                    disabled={action.busy}
                    onChange={setOurs}
                  />
                ) : domain.oursSince ? (
                  "On"
                ) : (
                  "Off"
                )
              }
            />
          </Rows>
          <Problem>{action.problem}</Problem>
        </div>
      )}
    </Row>
  );
}

function Waiting({
  org,
  domain,
  open,
  missing,
  onChanged,
  onMissing,
}: {
  org: Org;
  domain: OrgDomain;
  open: boolean;
  missing: boolean;
  onChanged: (text: string) => void;
  onMissing: (words?: string) => void;
}) {
  const client = useClient();
  const cheer = useCheer();
  const action = useAction();
  const [removing, setRemoving] = useState(false);
  const verify = () => {
    onMissing();
    void action.run(
      async () => {
        await (
          await client("verify a domain")
        ).verifyOrgDomain(org.id, domain.id);
        cheer();
        onChanged(`${domain.domain} is verified`);
      },
      (error) => {
        const words = problemText(error);
        onMissing(words);
        return words;
      },
    );
  };
  return (
    <Row
      wrap
      lead={lead}
      name={
        <>
          {domain.domain}
          {missing ? (
            <Chip tone="bad">Not found</Chip>
          ) : (
            <Chip tone="warn">Not verified</Chip>
          )}
        </>
      }
      detail="Nobody can join through it yet."
      end={
        removing ? null : (
          <>
            {open ? (
              <Btn small busy={action.busy} onClick={verify}>
                {missing ? "Check again" : "Verify"}
              </Btn>
            ) : null}
            <Txt bad onClick={() => setRemoving(true)}>
              Remove
            </Txt>
          </>
        )
      }
    >
      {removing ? (
        <RemoveDomain
          org={org}
          domain={domain}
          onChanged={onChanged}
          onCancel={() => setRemoving(false)}
        />
      ) : (
        <>
          <dl className="kv">
            <Line label="Name" value={domain.record.name} />
            <Line label="Value" value={domain.record.value} />
          </dl>
          <Problem>{missing ? undefined : action.problem}</Problem>
        </>
      )}
    </Row>
  );
}

function Line({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <>
      <dt>{label}</dt>
      <dd className="mono">{value}</dd>
      <Txt
        aria-label={`Copy the record's ${label.toLowerCase()}`}
        onClick={() =>
          void navigator.clipboard.writeText(value).then(
            () => setCopied(true),
            () => setCopied(false),
          )
        }
      >
        {copied ? "Copied" : "Copy"}
      </Txt>
    </>
  );
}

function RemoveDomain({
  org,
  domain,
  onChanged,
  onCancel,
}: {
  org: Org;
  domain: OrgDomain;
  onChanged: (text: string) => void;
  onCancel: () => void;
}) {
  const client = useClient();
  const action = useAction();
  return (
    <Confirm
      danger
      busy={action.busy}
      problem={action.problem}
      label={`Remove ${domain.domain}`}
      onCancel={onCancel}
      onConfirm={() =>
        void action.run(
          async () => {
            await (
              await client("remove a domain")
            ).removeOrgDomain(org.id, domain.id);
            onChanged(`Removed ${domain.domain}`);
          },
          (error) => `${problemText(error)} It is still there.`,
        )
      }
    >
      <p>
        Nobody else can join {org.name} through it. Everyone who already did
        stays in.
      </p>
    </Confirm>
  );
}
