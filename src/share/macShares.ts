import { useEffect } from "react";
import { create } from "zustand";
import { onCoreChanged } from "../api/coreChanged";
import { shareApi, type ShareView } from "../api/share";
import { swallow } from "../state/toast";

export const SHARE_LIFETIMES = [
    { label: "1 hour", ms: 60 * 60_000 },
    { label: "24 hours", ms: 24 * 60 * 60_000 },
    { label: "7 days", ms: 7 * 24 * 60 * 60_000 },
] as const;

export const DEFAULT_SHARE_LIFETIME_MS = SHARE_LIFETIMES[1].ms;

interface SharesState {
    shares: readonly ShareView[];
    /** The chat whose share dialog is open. */
    dialog: string | null;
}

export const useShares = create<SharesState>(() => ({ shares: [], dialog: null }));

function same(a: ShareView, b: ShareView): boolean {
    return (
        a.id === b.id &&
        a.agentId === b.agentId &&
        a.url === b.url &&
        a.expiresAt === b.expiresAt &&
        a.viewers === b.viewers &&
        a.state === b.state &&
        a.problem === b.problem
    );
}

/** Reads the shares again. What did not change keeps its object, so a header showing it is not drawn again. */
export async function refreshShares(): Promise<void> {
    const read = await shareApi.list();
    const held = useShares.getState().shares;
    const shares = read.map((share) => held.find((old) => same(old, share)) ?? share);
    if (shares.length === held.length && shares.every((share, index) => share === held[index])) return;
    useShares.setState({ shares });
}

export function openShareDialog(agentId: string) {
    useShares.setState({ dialog: agentId });
}

export function closeShareDialog() {
    useShares.setState({ dialog: null });
}

export function useChatShare(agentId: string): ShareView | null {
    return useShares((state) => state.shares.find((share) => share.agentId === agentId && share.state !== "refused") ?? null);
}

/** Keeps the shares as the core has them, reading them again whenever the core says they changed. Mounted once, at the app's root. */
export function useFollowShares() {
    useEffect(() => {
        const read = () => void refreshShares().catch(swallow("read the shares"));
        read();
        return onCoreChanged("shares", read);
    }, []);
}
