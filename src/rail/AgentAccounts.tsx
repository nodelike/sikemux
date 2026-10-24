import { useEffect, useMemo, useRef, useState } from "react";
import type { AgentUsage } from "../api/agents";
import { accountName, accountsOf, openAccountDialog, signOut, switchAccount } from "../agents/accounts";
import { selectedProviderProfile } from "../agents/agentProfiles";
import { agentLabel, type AgentType } from "../agents/catalog";
import { fetchResource, invalidate, peekResource, type ResourceHandle, useResource } from "../state/resources";
import { agentAccountR, agentUsageR } from "../state/resources.defs";
import { useStore } from "../state/store";
import type { ProviderProfile } from "../state/types";
import { IconChevron, IconLogOut, IconPlus, IconRefresh, IconUser } from "../ui/Icons";
import { reportError, swallow } from "../state/toast";
import { CountUp } from "../ui/RollingText";
import { Tooltip } from "../ui/Tooltip";
import { TreeContextMenu, type CtxItem } from "./FileTree";
import { planLabel, resetCountdown, resetTitle, usagePeak, usageTone } from "./usageFormat";
import { useNow } from "../hooks/useNow";

/**
 * The rail's footer: the plan limits of the account new chats start on. The
 * account is named in the head, which opens the person's other accounts with
 * the provider.
 */
export function AgentAccountsPanel({
    provider,
    usage,
}: {
    provider: AgentType;
    /** The limits of the account new chats start on, which the rail already reads. */
    usage: ResourceHandle<AgentUsage>;
}) {
    const profiles = useStore((s) => s.providerProfiles);
    const selections = useStore((s) => s.selectedProviderProfileIds);
    const accounts = useMemo(() => accountsOf(provider, profiles), [provider, profiles]);
    const current = selectedProviderProfile(provider, profiles, selections) ?? accounts[0];
    const [menu, setMenu] = useState<{ x: number; y: number; width: number } | null>(null);
    const head = useRef<HTMLDivElement>(null);
    const providerLabel = agentLabel(provider);
    const status = useResource(agentAccountR, provider, current?.configPath);
    const signedOut = status.data?.signedIn === false;

    /* The menu names each other account by who it is signed in as and how
       much it has used, read ahead so it is there when the menu opens. */
    const readOthers = () => {
        for (const account of accounts) {
            if (account.id === current?.id) continue;
            void fetchResource(agentAccountR, provider, account.configPath)
                .then((other) => (other.signedIn ? fetchResource(agentUsageR, provider, account.configPath) : undefined))
                .catch(swallow("read another account's usage"));
        }
    };
    useEffect(readOthers, [accounts.length]); // eslint-disable-line react-hooks/exhaustive-deps

    const refresh = () => invalidate((kind, args) => (kind === "agents.account" || kind === "agents.usage") && args[0] === provider);

    if (!current) return null;
    const name = accountName(current, status.data);
    const plan = usage.data?.plan ?? status.data?.plan;
    const items = accountMenu(provider, accounts, current, usage.data);

    return (
        <section className={`agent-usage agent-accounts ${provider}`} aria-label={`${providerLabel} plan limits`}>
            <div ref={head} className="panel-head agent-usage-head">
                <span className="panel-label">Limits</span>
                <span className="panel-rule" />
                <button
                    type="button"
                    className="agent-account-switch"
                    aria-haspopup="menu"
                    aria-expanded={menu !== null}
                    aria-label={`${providerLabel} account: ${name}`}
                    title={signedOut ? "Signed out" : [status.data?.email, plan && planLabel(plan)].filter(Boolean).join(" · ") || undefined}
                    onClick={() => {
                        readOthers();
                        const box = head.current!.getBoundingClientRect();
                        setMenu({ x: box.left, y: box.top - 6, width: box.width });
                    }}>
                    <span className="agent-account-name">{name}</span>
                    <IconChevron size={9} className="agent-account-chevron" />
                </button>
                <Tooltip label={`Refresh ${providerLabel} plan limits`}>
                    <button
                        type="button"
                        className="rail-group-add"
                        aria-label={`Refresh ${providerLabel} plan limits`}
                        disabled={usage.status === "loading"}
                        onClick={refresh}>
                        <IconRefresh size={11} />
                    </button>
                </Tooltip>
            </div>
            {menu && (
                <TreeContextMenu
                    x={menu.x}
                    y={menu.y}
                    width={menu.width}
                    above
                    className="agent-account-menu"
                    tickBesideLabel
                    items={items}
                    onClose={() => setMenu(null)}
                />
            )}

            {signedOut ? (
                <div className="agent-usage-empty">
                    {name} is signed out.{" "}
                    <button type="button" className="agent-usage-link" onClick={() => openAccountDialog({ provider, profileId: current.id })}>
                        Sign in
                    </button>
                </div>
            ) : (
                <UsageWindows usage={usage} />
            )}
        </section>
    );
}

/* Which signed-in account new chats use, and a new one to sign in. Accounts
   signed out of wait in the sign-in dialog, where they can be removed. */
function accountMenu(provider: AgentType, accounts: ProviderProfile[], current: ProviderProfile, currentUsage: AgentUsage | undefined): CtxItem[] {
    const shown = accounts.filter(
        (account) => account.id === current.id || peekResource(agentAccountR, provider, account.configPath)?.signedIn !== false,
    );
    const items: CtxItem[] = shown.map((account) => {
        const selected = account.id === current.id;
        const item = accountItem(provider, account, selected, selected ? currentUsage : undefined);
        const name = item.label!;
        const signedOut = peekResource(agentAccountR, provider, account.configPath)?.signedIn === false;
        return {
            ...item,
            run: selected ? undefined : () => void switchAccount(provider, account).catch(reportError("Switch account")),
            actions: [
                signedOut
                    ? { label: `Sign in to ${name}`, icon: <IconUser size={13} />, run: () => openAccountDialog({ provider, profileId: account.id }) }
                    : {
                          label: `Sign out of ${name}`,
                          icon: <IconLogOut size={13} />,
                          run: () => void signOut(account).catch(reportError("Sign out")),
                      },
            ],
        };
    });
    items.push({ sep: true }, { label: "Add account…", icon: <IconPlus size={13} />, run: () => openAccountDialog({ provider }) });
    return items;
}

/** One account as an account menu draws it: its mark, who it is signed in as and how much of its plan it has used. */
export function accountItem(provider: AgentType, account: ProviderProfile, selected: boolean, usage?: AgentUsage): CtxItem {
    const status = peekResource(agentAccountR, provider, account.configPath);
    const peak = usagePeak(usage ?? peekResource(agentUsageR, provider, account.configPath));
    const signedOut = status?.signedIn === false;
    const name = accountName(account, status);
    return {
        label: name,
        detail: signedOut ? "Signed out" : (status?.email ?? status?.organization ?? undefined),
        hint: !signedOut && peak != null ? `${Math.round(peak)}%` : undefined,
        selected,
        icon: (
            <span className="agent-account-mark" style={{ background: account.accent }} aria-hidden="true">
                {name.trim().charAt(0).toUpperCase() || "?"}
            </span>
        ),
    };
}

function UsageWindows({ usage }: { usage: ResourceHandle<AgentUsage> }) {
    const now = useNow(true, 60_000);
    const windows = usage.data?.windows ?? [];
    if (windows.length === 0) {
        return (
            <div className="agent-usage-empty" data-loading={usage.status === "loading" ? "true" : "false"}>
                {usage.status === "loading"
                    ? "reading plan limits…"
                    : usage.status === "error"
                      ? "Could not read plan limits. Refresh to try again."
                      : (usage.data?.unavailableReason ?? "This account did not report plan limits.")}
            </div>
        );
    }
    return (
        <>
            {windows.map((window, index) => {
                const percent = Math.max(0, Math.min(100, window.usedPercent));
                const rounded = Math.round(percent);
                return (
                    <Tooltip
                        key={`${window.label}:${String(window.resetsAt)}:${index}`}
                        side="left"
                        label={`${window.label}: ${rounded}% used. ${resetTitle(window.resetsAt)}`}>
                        <div className="agent-usage-row" data-tone={usageTone(percent)}>
                            <span className="agent-usage-name">{window.label}</span>
                            <span
                                className="agent-usage-track"
                                role="meter"
                                aria-label={`${window.label} usage`}
                                aria-valuemin={0}
                                aria-valuemax={100}
                                aria-valuenow={rounded}>
                                <span className="agent-usage-fill" style={{ width: `${percent}%` }} />
                            </span>
                            <span className="agent-usage-pct">
                                <CountUp value={rounded} />%
                            </span>
                            <span className="agent-usage-reset">{resetCountdown(window.resetsAt, now).replace(/^reset /, "")}</span>
                        </div>
                    </Tooltip>
                );
            })}
        </>
    );
}
