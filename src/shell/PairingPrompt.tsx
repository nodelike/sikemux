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

function usePendingDevices(hasFocus: () => boolean): [readonly PendingDevice[], (next: Promise<RemoteStatus>) => Promise<void>] {
    const [pending, setPending] = useState<readonly PendingDevice[]>([]);
    const announced = useRef(new Set<string>());
    useEffect(() => {
        const controller = new AbortController();
        const show = (status: RemoteStatus) => {
            if (!controller.signal.aborted) setPending(status.pending);
        };
        remoteApi
            .subscribe(show, controller.signal)
            .then(() => remoteApi.status())
            .then(show)
            .catch((error: unknown) => {
                if (!controller.signal.aborted) swallow("pairing requests")(error);
            });
        return () => controller.abort();
    }, []);
    useEffect(() => {
        const fresh = pending.filter((request) => !announced.current.has(request.id));
        announced.current = new Set(pending.map((request) => request.id));
        if (fresh.length === 0 || hasFocus()) return;
        getCurrentWindow().requestUserAttention(UserAttentionType.Critical).catch(swallow("bounce dock icon"));
        for (const request of fresh) {
            const { title, body } = pairingNotification(request);
            postNotification(title, body).catch(swallow("pairing notification"));
        }
    }, [pending, hasFocus]);
    const answer = async (next: Promise<RemoteStatus>) => {
        try {
            setPending((await next).pending);
        } catch (error) {
            reportError("Pairing")(error);
        }
    };
    return [pending, answer];
}

const documentHasFocus = () => document.hasFocus();

/**
 * Asks about the phone that has waited longest, over the whole window, since a phone gives up
 * after two minutes. The next one waiting follows once it is answered. Right after signing in, it
 * first asks once about every phone already on the account, including any of them already waiting.
 */
export function PairingPrompt({ hasFocus = documentHasFocus }: { hasFocus?: () => boolean }) {
    const [pending, answer] = usePendingDevices(hasFocus);
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
    const oldest = pending.reduce<PendingDevice | null>((first, request) => (!first || request.expiresAt < first.expiresAt ? request : first), null);
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
