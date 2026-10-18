import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { getCurrentWindow, UserAttentionType } from "@tauri-apps/api/window";
import { postNotification } from "../agents/agentNotifications";
import { doneWithAccountPhones, useAccount } from "../account/account";
import { remoteApi, type DeviceAccess, type PendingDevice, type RemoteStatus } from "../api/remote";
import { reportError, swallow } from "../state/toast";

const ConnectTakeover = lazy(() => import("./ConnectTakeover"));

/** The notification for a device that starts waiting while Sikemux is in the background. */
export function pairingNotification(request: PendingDevice): { title: string; body: string } {
    return {
        title: `${request.name || "A phone"} wants to connect to this computer`,
        body: "It is signed in to your Sikemux account. Allow or decline it in Sikemux.",
    };
}

/** The notification for another host on the account that asks to link while Sikemux is in the background. */
export function linkNotification(request: PendingDevice): { title: string; body: string } {
    return {
        title: `${request.name || "Another computer"} wants to link with this computer`,
        body: "Its agents could start agents here. Allow or decline it in Sikemux.",
    };
}

interface Pending {
    phones: readonly PendingDevice[];
    links: readonly PendingDevice[];
}

function usePendingDevices(hasFocus: () => boolean): [Pending, (next: Promise<RemoteStatus>) => Promise<void>] {
    const [phones, setPhones] = useState<readonly PendingDevice[]>([]);
    const [links, setLinks] = useState<readonly PendingDevice[]>([]);
    const announced = useRef(new Set<string>());
    useEffect(() => {
        const controller = new AbortController();
        const show = (status: RemoteStatus) => {
            if (!controller.signal.aborted) setPhones(status.pending);
        };
        const readLinks = () =>
            void remoteApi
                .pairing()
                .then((pairing) => {
                    if (!controller.signal.aborted) setLinks(pairing.pendingLinks);
                })
                .catch(swallow("link requests"));
        remoteApi
            .subscribe((status) => {
                show(status);
                readLinks();
            }, controller.signal)
            .then(() => {
                readLinks();
                return remoteApi.status();
            })
            .then(show)
            .catch((error: unknown) => {
                if (!controller.signal.aborted) swallow("pairing requests")(error);
            });
        return () => controller.abort();
    }, []);
    useEffect(() => {
        const waiting = [
            ...phones.map((request) => ({ request, word: pairingNotification })),
            ...links.map((request) => ({ request, word: linkNotification })),
        ];
        const fresh = waiting.filter(({ request }) => !announced.current.has(request.id));
        announced.current = new Set(waiting.map(({ request }) => request.id));
        if (fresh.length === 0 || hasFocus()) return;
        getCurrentWindow().requestUserAttention(UserAttentionType.Critical).catch(swallow("bounce dock icon"));
        for (const { request, word } of fresh) {
            const { title, body } = word(request);
            postNotification(title, body).catch(swallow("pairing notification"));
        }
    }, [phones, links, hasFocus]);
    const answer = async (next: Promise<RemoteStatus>) => {
        try {
            setPhones((await next).pending);
        } catch (error) {
            reportError("Pairing")(error);
        }
    };
    return [{ phones, links }, answer];
}

function oldestOf(requests: readonly PendingDevice[]): PendingDevice | null {
    return requests.reduce<PendingDevice | null>((first, request) => (!first || request.expiresAt < first.expiresAt ? request : first), null);
}

const documentHasFocus = () => document.hasFocus();

/**
 * Asks about the phone or host that has waited longest, over the whole window, since each gives up
 * after two minutes. The next one waiting follows once it is answered. Right after signing in, it
 * first asks once about every phone already on the account, including any of them already waiting.
 */
export function PairingPrompt({ hasFocus = documentHasFocus }: { hasFocus?: () => boolean }) {
    const [{ phones: pending, links }, answer] = usePendingDevices(hasFocus);
    const phones = useAccount((s) => s.phonesToAllow);
    if (phones.length > 0) {
        const keys = new Set(phones.map((phone) => phone.key));
        const waiting = pending.filter((request) => keys.has(request.deviceId));
        const answerAll = async (allow: boolean, access: DeviceAccess) => {
            const devices = phones.map((phone) => ({ id: phone.key, name: phone.name, platform: phone.platform, access }));
            if (allow) await answer(remoteApi.allowDevices(devices));
            else for (const request of waiting) await answer(remoteApi.answerPairing(request.id, false, access));
            doneWithAccountPhones();
        };
        return (
            <Suspense fallback={null}>
                <ConnectTakeover key="signed-in" question={{ kind: "signedIn", names: phones.map((phone) => phone.name) }} onAnswer={answerAll} />
            </Suspense>
        );
    }
    const oldest = oldestOf(pending);
    const link = oldestOf(links);
    if (link && (!oldest || link.expiresAt < oldest.expiresAt)) {
        return (
            <Suspense fallback={null}>
                <ConnectTakeover
                    key={link.id}
                    question={{ kind: "link", request: link }}
                    onAnswer={(allow) => answer(remoteApi.answerPairing(link.id, allow, "full"))}
                />
            </Suspense>
        );
    }
    if (!oldest) return null;
    return (
        <Suspense fallback={null}>
            <ConnectTakeover
                key={oldest.id}
                question={{ kind: "waiting", request: oldest }}
                onAnswer={(allow, access) => answer(remoteApi.answerPairing(oldest.id, allow, access))}
            />
        </Suspense>
    );
}
