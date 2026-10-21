import { useState } from "react";
import { notify, reportError } from "../../plugin-api/host";
import { invalidate, useResourceEnabled } from "../../plugin-api/resources";
import { IconCheck, IconGit, IconLogOut, IconPlus, IconUser, Tooltip } from "../../plugin-api/ui";
import { useAccount, useHost, type HostAccountEntry } from "../registry";
import { accountsR, hostStatusR } from "../resources";
import { forgetAccount, setProjectAccount } from "../state";
import { Avatar, Initial } from "./Pictures";
import "../strip.css";

function AccountRow({ entry, current, onPick }: { entry: HostAccountEntry; current: boolean; onPick: () => void }) {
    return (
        <button
            type="button"
            className={`env-dd-item host-account-row${current ? " active" : ""}`}
            role="menuitemradio"
            aria-checked={current}
            onClick={onPick}>
            <span className="tree-ctx-icon">
                {entry.avatarUrl ? <Avatar url={entry.avatarUrl} login={entry.login} /> : <Initial login={entry.login} />}
            </span>
            <span className="host-account-name">
                {entry.login}
                {entry.detail && <span className="host-account-detail">{entry.detail}</span>}
            </span>
            {current && <IconCheck size={12} />}
        </button>
    );
}

/** Who is signed in to the code host, with the menu to switch accounts or repositories. */
export function HostAccountButton({
    slug,
    cwd,
    active,
    onSignIn,
    onPickRepo,
    onAddAccount,
}: {
    slug: string | null;
    /** The project folder, which remembers the account picked for it. */
    cwd: string;
    active: boolean;
    /** Absent where the screen below is already the sign-in form. */
    onSignIn: (() => void) | null;
    onPickRepo: () => void;
    onAddAccount: () => void;
}) {
    const host = useHost();
    const chosen = useAccount();
    const status = useResourceEnabled(active, hostStatusR, host.id, chosen);
    const account = status.data;
    const [menuOpen, setMenuOpen] = useState(false);
    const accounts = useResourceEnabled(active && menuOpen, accountsR, host.id).data ?? [];
    const current = account?.id ?? null;
    const isDefault = accounts.find((entry) => entry.id === current)?.isDefault ?? true;

    const act = (work: () => void) => {
        setMenuOpen(false);
        work();
    };

    const signOut = () =>
        host.api
            .signOut(current)
            .then(() => {
                if (current) forgetAccount(host.id, current);
                notify("success", `Signed ${account?.login ?? "out"} out of ${host.name}`);
                invalidate((kind) => kind.startsWith("host."));
            })
            .catch(reportError("Could not sign out"));

    const makeDefault = () =>
        current &&
        host.api
            .setDefaultAccount(current)
            .then(() => {
                notify("success", `New projects open as ${account?.login ?? "this account"}`);
                invalidate((kind) => kind === "host.accounts" || kind === "host.accountFor");
            })
            .catch(reportError("Could not change the default account"));

    return (
        <span className="git-nav-account">
            {account?.ok ? (
                <Tooltip label={`${account.login} on ${host.name}${slug ? ` · ${slug}` : ""}`}>
                    <button
                        type="button"
                        className="git-nav-avatar"
                        aria-label={`${host.name} account`}
                        aria-haspopup="menu"
                        aria-expanded={menuOpen}
                        onClick={() => setMenuOpen((was) => !was)}>
                        {account.avatarUrl ? <Avatar url={account.avatarUrl} /> : host.icon(16)}
                    </button>
                </Tooltip>
            ) : (
                onSignIn && (
                    <Tooltip label={`Sign in to ${host.name}`}>
                        <button type="button" className="git-nav-signin" aria-label={`Sign in to ${host.name}`} onClick={onSignIn}>
                            <span className="git-nav-signin-mark">{host.icon(12)}</span>
                            <span className="git-nav-signin-label">Sign in</span>
                        </button>
                    </Tooltip>
                )
            )}
            {menuOpen && account?.ok && (
                <>
                    <div className="env-dd-scrim" onClick={() => setMenuOpen(false)} />
                    <div className="env-dd-menu git-nav-menu" role="menu">
                        <div className="host-account-who">
                            {account.login} on {account.host}
                            {slug && <div className="git-nav-repo">{slug}</div>}
                        </div>
                        {account.warning && <div className="host-account-warning">{account.warning}</div>}
                        {accounts.length > 1 &&
                            accounts.map((entry) => (
                                <AccountRow
                                    key={entry.id}
                                    entry={entry}
                                    current={entry.id === current}
                                    onPick={() => act(() => setProjectAccount(host.id, cwd, entry.id))}
                                />
                            ))}
                        <button type="button" className="env-dd-item" role="menuitem" onClick={() => act(onAddAccount)}>
                            <span className="tree-ctx-icon">
                                <IconPlus size={13} />
                            </span>
                            Add another account…
                        </button>
                        <button
                            type="button"
                            className="env-dd-item"
                            role="menuitem"
                            onClick={() => {
                                setMenuOpen(false);
                                onPickRepo();
                            }}>
                            <span className="tree-ctx-icon">
                                <IconGit size={13} />
                            </span>
                            Choose another repository…
                        </button>
                        {accounts.length > 1 && !isDefault && (
                            <button type="button" className="env-dd-item" role="menuitem" onClick={() => act(() => void makeDefault())}>
                                <span className="tree-ctx-icon">
                                    <IconUser size={13} />
                                </span>
                                Open new projects as {account.login}
                            </button>
                        )}
                        <button type="button" className="env-dd-item" role="menuitem" onClick={() => act(() => void signOut())}>
                            <span className="tree-ctx-icon">
                                <IconLogOut size={13} />
                            </span>
                            Sign {account.login} out
                        </button>
                    </div>
                </>
            )}
        </span>
    );
}
