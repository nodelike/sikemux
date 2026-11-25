import type {
  Org,
  OrgDomain,
  OrgDomainJoining,
  TeamAccess,
} from "@sikemux/protocol";
import { formatDay } from "@shared/lib/format";
import {
  IconCheck,
  IconCopy,
  IconHourglass,
  IconMore,
  IconPause,
} from "@shared/ui/Icons";
import { useState } from "react";

import { problemText } from "./api.ts";
import { useClient } from "./client.ts";
import { useCheer } from "./Field.tsx";
import { iconMark } from "./marks.ts";
import "./members.css";
import { changeable, frozen, possessive, renewable, useLoad } from "./orgs.ts";
import { orgScene } from "./scenes.ts";
import { copyText } from "./Seats.tsx";
import {
  Btn,
  Confirm,
  Nothing,
  Notice,
  Page,
  Popover,
  Problem,
  Seal,
  Section,
  Seg,
  Switch,
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

export function DomainsPage({ org, team }: { org: Org; team: TeamAccess }) {
  const client = useClient();
  const say = useToast();
  const [missingId, setMissingId] = useState<string>();
  const loaded = useLoad(
    async () => (await client("see your domains")).listOrgDomains(org.id),
    org.role,
  );
  const open = changeable(org, team);
  const ended = renewable(org, team);
  const scene = orgScene(org.id);
  const mark = iconMark("at");
  const domains = loaded.data?.domains;
  const verified = domains?.filter((each) => each.verifiedAt) ?? [];
  const waiting = domains?.filter((each) => !each.verifiedAt) ?? [];

  const changed = (text: string) => {
    say(text);
    setMissingId(undefined);
    loaded.again();
  };

  if (loaded.problem)
    return (
      <Page
        title="Domains"
        lede="Who can find the org by their address."
        mark={mark}
        scene={scene}
        tone="failed"
      >
        <Nothing
          title="Domains didn't load"
          actions={<Btn onClick={loaded.again}>Try again</Btn>}
        >
          {loaded.problem} Nothing in the org has changed.
        </Nothing>
      </Page>
    );

  const notice = frozen(org) ? (
    ended ? (
      <Notice
        icon={<IconHourglass size={17} />}
        tone="cold"
        title={`${org.name} is frozen`}
      >
        Its Team plan ended. You can still remove a domain.
      </Notice>
    ) : (
      <Notice icon={<IconPause size={17} />} title={`${org.name} is paused`}>
        Team isn't open to your account now. You can still remove a domain.
      </Notice>
    )
  ) : !open && domains ? (
    <Notice
      icon={<IconPause size={17} />}
      title="Team isn't open to your account now"
    >
      You can still remove a domain.
    </Notice>
  ) : null;

  return (
    <Page
      title="Domains"
      lede="Who can find the org by their address."
      notice={notice}
      mark={mark}
      scene={scene}
      tone={missingId ? "failed" : frozen(org) ? "waiting" : undefined}
    >
      {!domains ? (
        <DomainsSkeleton />
      ) : domains.length === 0 ? (
        <Nothing
          title={open ? "Let colleagues in" : "No domains"}
          actions={
            open ? (
              <div className="m-empty-add">
                <AddDomain org={org} onAdded={changed} />
              </div>
            ) : undefined
          }
        >
          {open
            ? "Prove your domain and people with an address there find the org by themselves."
            : ended
              ? "The org is frozen. Domains come back when an owner renews Team."
              : frozen(org)
                ? `${org.name} is paused. Team isn't open to your account now. Nothing is deleted.`
                : "Team isn't open to your account now."}
        </Nothing>
      ) : (
        <div className="m-doms">
          {waiting.map((domain) => (
            <WaitingDomain
              key={domain.id}
              org={org}
              domain={domain}
              open={open}
              missing={missingId === domain.id}
              onChanged={changed}
              onMissing={(failed) =>
                setMissingId(failed ? domain.id : undefined)
              }
            />
          ))}
          {verified.map((domain) => (
            <VerifiedDomain
              key={domain.id}
              org={org}
              domain={domain}
              open={open}
              onChanged={changed}
            />
          ))}
          {open ? (
            <Section label="Another domain" className="m-dom-another">
              <AddDomain org={org} onAdded={changed} />
            </Section>
          ) : null}
        </div>
      )}
    </Page>
  );
}

function DomainsSkeleton() {
  return (
    <div className="m-sklist m-dom-sk" aria-busy="true">
      {Array.from({ length: 2 }, (_, at) => (
        <div key={at}>
          <span className="skel m-skel-av" />
          <span>
            <span
              className="skel m-skel-bar"
              style={{ width: 160 + at * 50 }}
            />
            <span
              className="skel m-skel-bar"
              style={{ width: 260 + at * 30 }}
            />
          </span>
        </div>
      ))}
    </div>
  );
}

function AddDomain({
  org,
  onAdded,
}: {
  org: Org;
  onAdded: (text: string) => void;
}) {
  const client = useClient();
  const action = useAction();
  const [domain, setDomain] = useState("");
  const [joining, setJoining] = useState<OrgDomainJoining>("offered");

  return (
    <form
      className="m-add-form"
      onSubmit={(event) => {
        event.preventDefault();
        const wanted = domain.trim();
        if (!wanted) return;
        void action.run(
          async () => {
            const added = await (
              await client("add a domain")
            ).addOrgDomain(org.id, { domain: wanted, joining });
            setDomain("");
            onAdded(`Added ${added.domain}. Put its TXT record in DNS next.`);
          },
          (error) => `${problemText(error)} No domain was added.`,
        );
      }}
    >
      <Problem>{action.problem}</Problem>
      <div className="m-add">
        <input
          className="field"
          placeholder="yourcompany.com"
          aria-label="Email domain"
          value={domain}
          maxLength={253}
          disabled={action.busy}
          onChange={(event) => setDomain(event.target.value)}
        />
        <Seg
          label="How they come in"
          value={joining}
          disabled={action.busy}
          onChange={setJoining}
          options={JOINING}
        />
        <Btn type="submit" busy={action.busy} disabled={!domain.trim()}>
          Add domain
        </Btn>
      </div>
    </form>
  );
}

function VerifiedDomain({
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
    <section className="m-dom">
      <div className="m-dh">
        <div className="m-dh-main">
          <h2>{domain.domain}</h2>
          <Seal tone="ok">
            Since {formatDay(domain.verifiedAt ?? domain.createdAt, "long")}
          </Seal>
        </div>
        <Popover
          look="icon"
          label={`More for ${domain.domain}`}
          trigger={<IconMore size={16} />}
          options={[
            {
              key: "remove",
              name: "Remove domain",
              bad: true,
              choose: () => setRemoving(true),
            },
          ]}
        />
      </div>

      <p>
        {domain.joining === "open"
          ? `People with a ${domain.domain} address join by themselves.`
          : `People with a ${domain.domain} address are offered ${org.name} when they sign in.`}
      </p>

      <Problem>{action.problem}</Problem>

      <div className="m-opt">
        <span>
          <b>How they come in</b>
          <small>
            {domain.joining === "open"
              ? "They join on a free seat, as Member."
              : "They are asked when they sign in, and join as Member."}
          </small>
        </span>
        {open ? (
          <Seg
            label="How they come in"
            value={domain.joining}
            disabled={action.busy}
            onChange={(joining) => {
              if (joining !== domain.joining) setJoining(joining);
            }}
            options={JOINING}
          />
        ) : (
          <span className="m-dim">{JOINING_WORDS[domain.joining]}</span>
        )}
      </div>

      <div className="m-opt">
        <span>
          <b>These addresses are ours</b>
          <small>
            {domain.oursSince
              ? `Since ${formatDay(domain.oursSince, "short")}, an address at ${domain.domain} can't sign up for its own Sikemux account or be added as a sign-in. People verify it under Work addresses instead. Accounts and sign-ins from before then stay.`
              : `Turn this on to stop new Sikemux accounts and sign-ins at ${domain.domain}. People then sign in with their own account and verify their ${domain.domain} address under Work addresses. Accounts and sign-ins that already exist stay.`}
          </small>
        </span>
        {open && org.role === "owner" ? (
          <Switch
            label="These addresses are ours"
            on={domain.oursSince !== null}
            disabled={action.busy}
            onChange={setOurs}
          />
        ) : (
          <span className="m-dim">{domain.oursSince ? "On" : "Off"}</span>
        )}
      </div>

      {removing ? (
        <RemoveDomainConfirm
          org={org}
          domain={domain}
          onChanged={onChanged}
          onCancel={() => setRemoving(false)}
        />
      ) : null}
    </section>
  );
}

function WaitingDomain({
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
  onMissing: (failed: boolean) => void;
}) {
  const client = useClient();
  const cheer = useCheer();
  const action = useAction();
  const [removing, setRemoving] = useState(false);

  const verify = () => {
    onMissing(false);
    void action.run(
      async () => {
        await (
          await client("verify a domain")
        ).verifyOrgDomain(org.id, domain.id);
        cheer();
        onChanged(`${domain.domain} is verified`);
      },
      (error) => {
        onMissing(true);
        return problemText(error);
      },
    );
  };

  return (
    <section className="m-dom">
      <div className="m-dh">
        <div className="m-dh-main">
          <h2>{domain.domain}</h2>
          <Seal tone={missing ? "bad" : "wait"}>
            {missing ? "No record yet" : "Waiting for its record"}
          </Seal>
        </div>
        <Popover
          look="icon"
          label={`More for ${domain.domain}`}
          trigger={<IconMore size={16} />}
          options={[
            {
              key: "remove",
              name: "Remove domain",
              bad: true,
              choose: () => setRemoving(true),
            },
          ]}
        />
      </div>

      <p>Nobody can join through it yet.</p>

      <ol className="m-steps">
        <li>
          <b>Add this TXT record where your domain lives</b>
          <dl>
            <RecordLine label="Name" value={domain.record.name} />
            <RecordLine label="Value" value={domain.record.value} />
          </dl>
        </li>
        <li>
          <b>Check for it</b>
          {action.problem ? (
            <div className="m-step-err">
              <Problem>{action.problem}</Problem>
            </div>
          ) : (
            <small>It can take up to an hour to show.</small>
          )}
          {open ? (
            <div className="m-act">
              <Btn busy={action.busy} onClick={verify}>
                {action.busy ? "Looking" : missing ? "Check again" : "Verify"}
              </Btn>
            </div>
          ) : null}
        </li>
      </ol>

      {removing ? (
        <RemoveDomainConfirm
          org={org}
          domain={domain}
          onChanged={onChanged}
          onCancel={() => setRemoving(false)}
        />
      ) : null}
    </section>
  );
}

function RecordLine({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div>
      <dt>{label}</dt>
      <dd>{value}</dd>
      <Btn
        small
        quiet
        className={copied ? "m-did" : undefined}
        aria-label={`Copy the record's ${label.toLowerCase()}`}
        onClick={() =>
          void copyText(value).then((ok) => {
            if (!ok) return;
            setCopied(true);
            setTimeout(() => setCopied(false), 1600);
          })
        }
      >
        {copied ? <IconCheck size={13} /> : <IconCopy size={13} />}
        {copied ? "Copied" : "Copy"}
      </Btn>
    </div>
  );
}

function RemoveDomainConfirm({
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
      title={`Remove ${domain.domain}?`}
      busy={action.busy}
      problem={action.problem}
      label="Remove domain"
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
