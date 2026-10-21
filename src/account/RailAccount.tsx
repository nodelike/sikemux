import { useEffect, useRef, useState } from "react";
import { accountApi } from "../api/account";
import { useShortcutLabel, withShortcut } from "../commands/useShortcutLabel";
import { TreeContextMenu, type CtxItem } from "../rail/FileTree";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { swallow } from "../state/toast";
import { IconClose, IconLaptop, IconLogIn, IconLogOut, IconSettings, IconUser } from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";
import { AccountAvatar } from "./AccountAvatar";
import { loadAccount, signInToAccount, signOutOfAccount, useAccount, watchAccount } from "./account";

/** The foot of the side rail: who is signed in, with their menu and settings, or a card asking them to sign in. */
export function RailAccount() {
    const account = useAccount((s) => s.account);
    const signingIn = useAccount((s) => s.signingIn);
    const dismissed = useStore((s) => s.signInCardDismissed);
    const settingsShortcut = useShortcutLabel("settings.toggle");
    const row = useRef<HTMLButtonElement>(null);
    const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);

    useEffect(() => {
        const controller = new AbortController();
        watchAccount(controller.signal).catch(swallow("account"));
        loadAccount().catch(swallow("account"));
        return () => controller.abort();
    }, []);

    if (!account) return null;

    if (!account.signedIn) {
        if (dismissed && !signingIn) return null;
        return (
            <section className="rail-sign-in">
                <p>{signingIn ? "Finish signing in in your browser." : "Sign in to reach this host from your phone and other devices."}</p>
                {!signingIn && (
                    <Tooltip label="Sign in later from Settings → Devices">
                        <button type="button" className="rail-sign-in-close" aria-label="Dismiss" onClick={cmd.dismissSignInCard}>
                            <IconClose size={11} />
                        </button>
                    </Tooltip>
                )}
                {signingIn ? (
                    <button type="button" className="rail-sign-in-btn" onClick={() => void accountApi.cancelSignIn()}>
                        Cancel
                    </button>
                ) : (
                    <button type="button" className="rail-sign-in-btn primary" onClick={() => void signInToAccount()}>
                        <IconLogIn size={13} />
                        Sign in
                    </button>
                )}
            </section>
        );
    }

    const email = account.email ?? "Signed in";
    const items: CtxItem[] = [
        { label: "Your account", icon: <IconUser size={13} />, run: () => cmd.openSettings("devices", "Your account") },
        { label: "Devices", icon: <IconLaptop size={13} />, run: () => cmd.openSettings("devices") },
        { label: "Settings", icon: <IconSettings size={13} />, hint: settingsShortcut || undefined, run: () => cmd.openSettings() },
        { sep: true },
        { label: "Sign out", icon: <IconLogOut size={13} />, run: () => void signOutOfAccount() },
    ];

    return (
        <div className="rail-account">
            <button
                ref={row}
                type="button"
                className={`rail-account-row${menu ? " open" : ""}`}
                aria-label={`Account: ${email}`}
                aria-haspopup="menu"
                aria-expanded={!!menu}
                onClick={() => {
                    const rect = row.current?.getBoundingClientRect();
                    if (rect) setMenu({ x: rect.left, y: rect.top - 4 });
                }}>
                <AccountAvatar account={account} className="rail-account-face" />
                <span className="rail-account-who">
                    <b>{account.name ?? email}</b>
                    {account.name && <span>{email}</span>}
                </span>
            </button>
            <Tooltip label={withShortcut("Settings", settingsShortcut)}>
                <button type="button" className="rail-account-settings" aria-label="Settings" onClick={cmd.toggleSettings}>
                    <IconSettings size={14} />
                </button>
            </Tooltip>
            {menu && <TreeContextMenu x={menu.x} y={menu.y} above width={row.current?.offsetWidth} items={items} onClose={() => setMenu(null)} />}
        </div>
    );
}
