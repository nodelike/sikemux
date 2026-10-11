import { useEffect, useMemo, useRef, useState } from "react";
import type { AgentAccountStatus, AgentUsage } from "../api/agents";
import { accountsOf, type AccountProvider } from "../agents/accounts";
import { accountsSectionTitle } from "../settings/AccountsSection";
import { selectedProviderProfile } from "../agents/agentProfiles";
import * as cmd from "../state/commands";
import { fetchResource, invalidate, peekResource, type ResourceHandle, useResource } from "../state/resources";
import { agentAccountR, agentUsageR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { DEFAULT_PROVIDER_PROFILES, type ProviderProfile } from "../state/types";
import { IconChevron, IconRefresh } from "../ui/Icons";
import { CountUp } from "../ui/RollingText";
import { Tooltip } from "../ui/Tooltip";
import { TreeContextMenu, type CtxItem } from "./FileTree";
import { planLabel, resetCountdown, resetTitle, usagePeak, usageTone } from "./usageFormat";

function useMinuteClock(): number {
    const [now, setNow] = useState(() => Date.now());
    useEffect(() => {
        const timer = window.setInterval(() => setNow(Date.now()), 60_000);
        return () => window.clearInterval(timer);
    }, []);
    return now;
}

function initials(name: string): string {
    const words = name.trim().split(/\s+/).filter(Boolean);
    const letters = words.length > 1 ? words[0][0] + words[1][0] : (words[0]?.slice(0, 2) ?? "");
    return letters.toUpperCase() || "?";
}

/* A profile still under its built-in name says nothing about whose account
   it is, so it goes by the name on the account instead. */
function accountName(account: ProviderProfile, status: AgentAccountStatus | undefined): string {
    const unnamed = DEFAULT_PROVIDER_PROFILES.some((builtin) => builtin.id === account.id && builtin.name === account.name);
    return (unnamed && status?.name) || account.name;
}

/**
 * The rail's footer: the plan limits of the account new chats start on. The
 * account is named in the head, which opens the person's other accounts with
 * the provider.
 */
export function AgentAccountsPanel({
    provider,
    usage,
    label,
    command,
}: {
    provider: AccountProvider;
    /** The limits of the account new chats start on, which the rail already reads. */
    usage: ResourceHandle<AgentUsage>;
    label?: string;
    command?: string;
}) {
    const profiles = useStore((s) => s.providerProfiles);
    const selections = useStore((s) => s.selectedProviderProfileIds);
    const accounts = useMemo(() => accountsOf(provider, profiles), [provider, profiles]);
    const current = selectedProviderProfile(provider, profiles, selections) ?? accounts[0];
    const [menu, setMenu] = useState<{ x: number; y: number; width: number } | null>(null);
    const head = useRef<HTMLDivElement>(null);
    const providerLabel = label ?? (provider === "claude" ? "Claude" : "Codex");
    const status = useResource(agentAccountR, provider, current?.executablePath, current?.configPath);
    const signedOut = status.data?.signedIn === false;

    /* The menu names each other account by who it is signed in as and how
       much it has used, read ahead so it is there when the menu opens. */
    const readOthers = () => {
        for (const account of accounts) {
            if (account.id === current?.id) continue;
            void fetchResource(agentAccountR, provider, account.executablePath, account.configPath)
                .then((other) => (other.signedIn ? fetchResource(agentUsageR, provider, command, account.configPath) : undefined))
                .catch(() => {});
        }
    };
    useEffect(readOthers, [accounts.length]); // eslint-disable-line react-hooks/exhaustive-deps

    const refresh = () => invalidate((kind, args) => (kind === "agents.account" || kind === "agents.usage") && args[0] === provider);

    if (!current) return null;
    const name = accountName(current, status.data);
    const plan = usage.data?.plan ?? status.data?.plan;
    const items = accountMenu(provider, accounts, current, usage.data, command);

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
                    title={signedOut ? "Signed out" : (status.data?.email ?? undefined)}
                    onClick={() => {
                        readOthers();
                        const box = head.current!.getBoundingClientRect();
                        setMenu({ x: box.left, y: box.top - 6, width: box.width });
                    }}>
                    <span className="agent-account-mark" aria-hidden="true">
                        {initials(name)}
                    </span>
                    <span className="agent-account-name">{name}</span>
                    {plan && <span className="agent-account-plan">{planLabel(plan)}</span>}
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
                    items={items}
                    onClose={() => setMenu(null)}
                />
            )}

            {signedOut ? (
                <div className="agent-usage-empty">
                    {name} is signed out.{" "}
                    <button type="button" className="agent-usage-link" onClick={() => manageAccounts(provider)}>
                        Sign in from Settings
                    </button>
                </div>
            ) : (
                <UsageWindows usage={usage} />
            )}
        </section>
    );
}

function manageAccounts(provider: AccountProvider): void {
    cmd.openSettings("agents", accountsSectionTitle(provider));
}

/* Only which account new chats use is chosen here; everything done to an
   account happens in Settings. */
function accountMenu(
    provider: AccountProvider,
    accounts: ProviderProfile[],
    current: ProviderProfile,
    currentUsage: AgentUsage | undefined,
    command?: string,
): CtxItem[] {
    const items: CtxItem[] = accounts.map((account) => {
        const selected = account.id === current.id;
        const status = peekResource(agentAccountR, provider, account.executablePath, account.configPath);
        const peak = usagePeak(selected ? currentUsage : peekResource(agentUsageR, provider, command, account.configPath));
        const signedOut = status?.signedIn === false;
        const name = accountName(account, status);
        return {
            label: name,
            detail: signedOut ? "Signed out" : (status?.email ?? status?.organization ?? undefined),
            hint: !signedOut && peak != null ? `${Math.round(peak)}%` : undefined,
            selected,
            icon: (
                <span className="agent-account-mark" aria-hidden="true">
                    {initials(name)}
                </span>
            ),
            run: selected ? undefined : () => cmd.selectProviderProfile(provider, account.id),
        };
    });
    items.push({ sep: true }, { label: "Manage accounts…", run: () => manageAccounts(provider) });
    return items;
}

function UsageWindows({ usage }: { usage: ResourceHandle<AgentUsage> }) {
    const now = useMinuteClock();
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
                            <div className="agent-usage-line">
                                <span className="agent-usage-name">{window.label}</span>
                                <span className="agent-usage-reset">{resetCountdown(window.resetsAt, now)}</span>
                            </div>
                            <div className="agent-usage-gauge">
                                <span className="agent-usage-pct">
                                    <CountUp value={rounded} />
                                    <i>%</i>
                                </span>
                                <span
                                    className="agent-usage-track"
                                    role="meter"
                                    aria-label={`${window.label} usage`}
                                    aria-valuemin={0}
                                    aria-valuemax={100}
                                    aria-valuenow={rounded}>
                                    <span className="agent-usage-fill" style={{ width: `${percent}%` }} />
                                </span>
                            </div>
                        </div>
                    </Tooltip>
                );
            })}
        </>
    );
}
