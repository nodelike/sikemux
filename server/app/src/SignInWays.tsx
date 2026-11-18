import { useAuth, useUser } from "@clerk/react";
import type { SignInRemoval } from "@sikemux/protocol";
import { IconMail, IconPlus } from "@shared/ui/Icons";
import {
  useEffect,
  useId,
  useRef,
  useState,
  type ReactNode,
  type SubmitEvent,
} from "react";

import { api, problemText, signedInToken } from "./api.ts";
import { AppleMark, GitHubMark, GoogleMark } from "./icons.tsx";
import { useReverified } from "./Reverify.tsx";
import { CodeEntry } from "./SignIn.tsx";
import { ItemRow, Rows, Section, useAction, useNotice } from "./ui.tsx";

type Way = {
  id: string;
  name: string;
  mark: ReactNode;
  detail: string;
  remove: (() => Promise<unknown>) | null;
};

export const PROVIDERS: Record<string, { name: string; mark: ReactNode }> = {
  google: { name: "Google", mark: <GoogleMark /> },
  apple: { name: "Apple", mark: <AppleMark /> },
  github: { name: "GitHub", mark: <GitHubMark /> },
};

const CONNECTABLE = ["google", "apple"] as const;

/** The person's own ways to sign in, with adding and removing behind a recent sign-in. */
export function SignInWaysSection() {
  const { user } = useUser();
  const { getToken } = useAuth();
  const { notice, show } = useNotice();
  const action = useAction();
  const [adding, setAdding] = useState(false);
  const { run, confirm } = useReverified(async (step: () => Promise<unknown>) =>
    step(),
  );
  const tidied = useRef(false);
  const [removals, setRemovals] = useState<SignInRemoval[]>([]);

  useEffect(() => {
    if (tidied.current || !user) return;
    tidied.current = true;
    void (async () => {
      const token = await getToken();
      if (!token) return;
      const { removed } = await api(token).tidySignIns();
      if (removed.length === 0) return;
      await user.reload();
      setRemovals(removed);
    })().catch(() => undefined);
  }, [user, getToken]);

  if (confirm) return confirm;
  if (!user) return null;

  const linked = new Set(
    user.emailAddresses.flatMap((address) =>
      address.linkedTo.length > 0 ? [address.id] : [],
    ),
  );
  const ways: Way[] = [
    ...user.externalAccounts.map((account) => {
      const provider = PROVIDERS[account.provider];
      return {
        id: account.id,
        name: provider?.name ?? account.provider,
        mark: provider?.mark ?? <IconMail size={16} />,
        detail: account.emailAddress,
        remove: () => account.destroy(),
      };
    }),
    ...user.emailAddresses
      .filter(
        (address) =>
          !linked.has(address.id) && address.verification.status === "verified",
      )
      .map((address) => ({
        id: address.id,
        name: address.emailAddress,
        mark: <IconMail size={16} />,
        detail:
          address.id === user.primaryEmailAddressId
            ? "Email code · your main address"
            : "Email code",
        remove:
          address.id === user.primaryEmailAddressId
            ? null
            : () => address.destroy(),
      })),
  ];
  const onlyOne = ways.length === 1;

  const remove = (way: Way, removing: () => Promise<unknown>) =>
    action.run(async () => {
      await run(removing);
      await user.reload();
      show(`${way.name} no longer signs in to your account.`);
    }, problemText);

  const connect = (strategy: (typeof CONNECTABLE)[number]) =>
    action.run(async () => {
      const back = new URL(location.href);
      const account = await run(() =>
        user.createExternalAccount({
          strategy: `oauth_${strategy}`,
          redirectUrl: back.toString(),
        }),
      );
      const next = (
        account as Awaited<ReturnType<typeof user.createExternalAccount>>
      ).verification?.externalVerificationRedirectURL;
      if (next) location.assign(next.toString());
    }, problemText);

  return (
    <Section label="Signs in with">
      {removals.map((removal) => (
        <p key={removal.removedAt + removal.domain} className="section-lede">
          {removalWords(removal)}
        </p>
      ))}
      <Rows>
        {ways.map((way) => (
          <ItemRow
            key={way.id}
            glyph={way.mark}
            name={way.name}
            detail={
              onlyOne
                ? `${way.detail} · your only way in, so add another before removing it`
                : way.detail
            }
            end={
              way.remove ? (
                <button
                  type="button"
                  className="button small quiet-danger"
                  disabled={onlyOne || action.busy}
                  title={
                    onlyOne
                      ? "Your account needs one way to sign in."
                      : undefined
                  }
                  onClick={() => {
                    if (way.remove) void remove(way, way.remove);
                  }}
                >
                  Remove
                </button>
              ) : null
            }
          />
        ))}
        {adding ? (
          <AddAddress
            run={run}
            onDone={async (email) => {
              setAdding(false);
              if (!email) return;
              await user.reload();
              show(`${email} now signs in to your account.`);
            }}
          />
        ) : (
          <ItemRow
            glyph={<IconPlus size={16} />}
            name="Add a way to sign in"
            end={
              <span className="actions">
                {CONNECTABLE.map((strategy) => (
                  <button
                    key={strategy}
                    type="button"
                    className="button small"
                    disabled={action.busy}
                    onClick={() => void connect(strategy)}
                  >
                    {PROVIDERS[strategy].mark}
                    {PROVIDERS[strategy].name}
                  </button>
                ))}
                <button
                  type="button"
                  className="button small"
                  disabled={action.busy}
                  onClick={() => setAdding(true)}
                >
                  <IconMail size={16} />
                  Email
                </button>
              </span>
            }
          />
        )}
      </Rows>
      {action.problem ? (
        <p className="problem" role="alert">
          {action.problem}
        </p>
      ) : null}
      {notice}
    </Section>
  );
}

function AddAddress({
  run,
  onDone,
}: {
  run: (step: () => Promise<unknown>) => Promise<unknown>;
  onDone: (email?: string) => void;
}) {
  const { user } = useUser();
  const { getToken } = useAuth();
  const formId = useId();
  const [email, setEmail] = useState("");
  const [code, setCode] = useState("");
  const [sent, setSent] = useState<{
    attempt: (code: string) => Promise<unknown>;
  }>();
  const action = useAction();

  const send = (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!user) return;
    void action.run(async () => {
      const typed = email.trim();
      const token = await signedInToken(getToken, "add an address");
      await api(token).checkSignInAddress({ email: typed });
      const address = (await run(() =>
        user.createEmailAddress({ email: typed }),
      )) as Awaited<ReturnType<NonNullable<typeof user>["createEmailAddress"]>>;
      await address.prepareVerification({ strategy: "email_code" });
      setSent({ attempt: (code) => address.attemptVerification({ code }) });
    }, problemText);
  };

  const verify = (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!sent || code.length < 6) return;
    void action.run(async () => {
      await sent.attempt(code);
      onDone(email.trim());
    }, problemText);
  };

  const buttons = (label: string) => (
    <span className="actions">
      <button
        type="button"
        className="button small"
        onClick={() => onDone()}
        disabled={action.busy}
      >
        Cancel
      </button>
      <button
        type="submit"
        form={formId}
        className="button small primary"
        disabled={action.busy}
        data-busy={action.busy || undefined}
      >
        {label}
      </button>
    </span>
  );

  return sent ? (
    <ItemRow
      glyph={<IconMail size={16} />}
      name={
        <>
          Enter the code sent to <span className="ink">{email.trim()}</span>
        </>
      }
      end={buttons("Verify")}
    >
      <form id={formId} className="row-code" onSubmit={verify}>
        <CodeEntry
          code={code}
          onChange={setCode}
          disabled={action.busy}
          failed={Boolean(action.problem)}
        />
      </form>
      {action.problem ? (
        <p className="problem" role="alert">
          {action.problem}
        </p>
      ) : null}
    </ItemRow>
  ) : (
    <ItemRow
      glyph={<IconMail size={16} />}
      name={
        <form id={formId} className="inline-form" onSubmit={send}>
          <input
            className="field"
            type="email"
            value={email}
            onChange={(event) => setEmail(event.target.value)}
            placeholder="you@example.com"
            aria-label="Email address"
            autoFocus
            disabled={action.busy}
          />
        </form>
      }
      end={buttons("Send code")}
    >
      {action.problem ? (
        <p className="problem" role="alert">
          {action.problem}
        </p>
      ) : null}
    </ItemRow>
  );
}

function removalWords(removal: SignInRemoval): string {
  const what = removal.provider
    ? `A ${PROVIDERS[removal.provider]?.name ?? removal.provider} sign-in at ${removal.domain}`
    : `An address at ${removal.domain}`;
  return removal.orgName
    ? `${what} was removed. ${removal.orgName} says addresses there are its own, so it can't sign in to your account.`
    : `${what} was removed. It's already verified as a work address, so it can't sign in to your account.`;
}
