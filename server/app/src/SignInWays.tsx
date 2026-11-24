import { useAuth, useUser } from "@clerk/react";
import type { SignInRemoval } from "@sikemux/protocol";
import { IconMail, IconPlus } from "@shared/ui/Icons";
import {
  useEffect,
  useRef,
  useState,
  type ReactNode,
  type SubmitEvent,
} from "react";

import { api, problemText, signedInToken } from "./api.ts";
import { AppleMark, GitHubMark, GoogleMark } from "./icons.tsx";
import { useReverified } from "./Reverify.tsx";
import { CodeEntry } from "./SignIn.tsx";
import {
  Btn,
  Card,
  Field,
  Problem,
  Row,
  Rows,
  Tile,
  Txt,
  useAction,
  useToast,
} from "./ui.tsx";

type Account = NonNullable<ReturnType<typeof useUser>["user"]>;

type Way = {
  id: string;
  kind: string;
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

/** The first letter and the domain, enough to tell your own addresses apart. */
const masked = (email: string) =>
  email.replace(/^(.)[^@]*(?=@)/, (_, first: string) => `${first}•••`);

export function waysOf(user: Account): Way[] {
  const linked = new Set(
    user.emailAddresses.flatMap((address) =>
      address.linkedTo.length > 0 ? [address.id] : [],
    ),
  );
  return [
    ...user.externalAccounts.map((account) => {
      const provider = PROVIDERS[account.provider];
      return {
        id: account.id,
        kind: account.provider,
        name: provider?.name ?? account.provider,
        mark: provider?.mark ?? <IconMail size={16} />,
        detail: masked(account.emailAddress),
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
        kind: "email",
        name: "Code by email",
        mark: <IconMail size={16} />,
        detail:
          address.id === user.primaryEmailAddressId
            ? `${masked(address.emailAddress)} · main address`
            : masked(address.emailAddress),
        remove:
          address.id === user.primaryEmailAddressId
            ? null
            : () => address.destroy(),
      })),
  ];
}

/** The person's own ways to sign in, with adding and removing behind a recent sign-in. */
export function SignInWaysCard() {
  const { user } = useUser();
  const { getToken } = useAuth();
  const say = useToast();
  const action = useAction();
  const [working, setWorking] = useState<string>();
  const [adding, setAdding] = useState(false);
  const { run, confirm } = useReverified(async (step: () => Promise<unknown>) =>
    step(),
  );
  const tidied = useRef(false);
  const [removals, setRemovals] = useState<SignInRemoval[]>([]);

  const tidy = async () => {
    const token = await signedInToken(getToken, "check your sign-ins");
    const { removed } = await api(token).tidySignIns();
    if (removed.length > 0) setRemovals((shown) => [...shown, ...removed]);
    return removed.length;
  };

  useEffect(() => {
    if (tidied.current || !user) return;
    tidied.current = true;
    void tidy()
      .then((removed) => (removed > 0 ? user.reload() : undefined))
      .catch(() => undefined);
  });

  if (confirm)
    return (
      <Card label="Signs in with" w={7}>
        {confirm}
      </Card>
    );
  if (!user) return null;

  const ways = waysOf(user);
  const onlyOne = ways.length === 1;

  const remove = (way: Way, removing: () => Promise<unknown>) => {
    setWorking(way.id);
    return action.run(async () => {
      await run(removing);
      await user.reload();
      say(
        way.kind === "email"
          ? `${way.detail} no longer signs in to your account.`
          : `${way.name} no longer signs in to your account.`,
      );
    }, problemText);
  };

  const connect = (strategy: (typeof CONNECTABLE)[number]) => {
    setWorking(strategy);
    return action.run(async () => {
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
  };

  return (
    <Card label="Signs in with" w={7}>
      {removals.map((removal) => (
        <p key={removal.removedAt + removal.domain} className="note">
          {removalWords(removal)}
        </p>
      ))}
      <Rows>
        {ways.map((way) => (
          <Row
            key={way.id}
            lead={
              <Tile>
                <span className="br">{way.mark}</span>
              </Tile>
            }
            name={way.name}
            detail={
              onlyOne ? `${way.detail}, and your only way in` : way.detail
            }
            end={
              way.remove ? (
                <Txt
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
                  {action.busy && working === way.id ? "Removing" : "Remove"}
                </Txt>
              ) : null
            }
          />
        ))}
        {CONNECTABLE.filter(
          (strategy) => !ways.some((way) => way.kind === strategy),
        ).map((strategy) => (
          <Row
            key={strategy}
            lead={
              <Tile>
                <span className="br">{PROVIDERS[strategy].mark}</span>
              </Tile>
            }
            name={PROVIDERS[strategy].name}
            detail="Not added"
            end={
              <Btn
                small
                quiet
                aria-label={`Add ${PROVIDERS[strategy].name}`}
                disabled={action.busy}
                busy={action.busy && working === strategy}
                onClick={() => void connect(strategy)}
              >
                Add
              </Btn>
            }
          />
        ))}
        {adding ? (
          <AddAddress
            run={run}
            onDone={async (email) => {
              setAdding(false);
              if (!email) return;
              const tidiedNow = await tidy().then(
                () => true,
                () => false,
              );
              const fresh = await user.reload();
              const kept = fresh.emailAddresses.some(
                (address) =>
                  address.emailAddress.toLowerCase() === email.toLowerCase() &&
                  address.verification.status === "verified",
              );
              if (tidiedNow && kept)
                say(`${masked(email)} now signs in to your account.`);
            }}
          />
        ) : (
          <Row
            lead={
              <Tile>
                <IconPlus size={16} />
              </Tile>
            }
            name={
              ways.some((way) => way.kind === "email")
                ? "Another email address"
                : "Code by email"
            }
            detail={
              ways.some((way) => way.kind === "email")
                ? "We send a code to it."
                : "Not added"
            }
            end={
              <Btn
                small
                quiet
                aria-label="Add an email address"
                disabled={action.busy}
                onClick={() => setAdding(true)}
              >
                Add
              </Btn>
            }
          />
        )}
      </Rows>
      <Problem>{action.problem}</Problem>
    </Card>
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

  return (
    <Row
      lead={
        <Tile>
          <IconMail size={16} />
        </Tile>
      }
      name={sent ? `Enter the code sent to ${email.trim()}` : "Email address"}
    >
      <form className="form below" onSubmit={sent ? verify : send}>
        {sent ? (
          <CodeEntry
            code={code}
            onChange={setCode}
            disabled={action.busy}
            failed={Boolean(action.problem)}
          />
        ) : (
          <Field
            type="email"
            value={email}
            onChange={(event) => setEmail(event.target.value)}
            placeholder="you@example.com"
            aria-label="Email address"
            autoFocus
            disabled={action.busy}
          />
        )}
        <Btn type="submit" small busy={action.busy}>
          {sent ? "Verify" : "Send code"}
        </Btn>
        <Btn small quiet disabled={action.busy} onClick={() => onDone()}>
          Cancel
        </Btn>
      </form>
      <Problem>{action.problem}</Problem>
    </Row>
  );
}

function removalWords(removal: SignInRemoval): string {
  const what = removal.provider
    ? `A ${PROVIDERS[removal.provider]?.name ?? removal.provider} sign-in at ${removal.domain}`
    : `An address at ${removal.domain}`;
  return removal.orgName
    ? `${what} was removed. Addresses there belong to ${removal.orgName}, so it can't sign in to your account.`
    : `${what} was removed. It's already verified as a work address, so it can't sign in to your account.`;
}
