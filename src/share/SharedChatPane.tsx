import { useEffect, useState } from "react";
import { errorText } from "@shared/lib/format";
import { shareApi } from "../api/share";
import { webCipher } from "@shared/share/cipher";
import { parseShareLink } from "@shared/share/link";
import { ShareSession } from "@shared/share/session";
import { SharedChat } from "./SharedChat";

/** A tab showing a chat someone shared, from the link the tab was opened with. */
export function SharedChatPane({ link, visible }: { paneId: string; link: string; visible: boolean }) {
    const [session, setSession] = useState<ShareSession | null>(null);
    const [problem, setProblem] = useState<string | null>(null);

    useEffect(() => {
        const parsed = parseShareLink(link);
        if (!parsed) {
            setProblem("This tab's share link is not one Sikemux can open.");
            return;
        }
        let live = true;
        let opened: ShareSession | null = null;
        shareApi
            .access()
            .then(({ socket }) => {
                if (!live) return;
                opened = new ShareSession({
                    link: parsed,
                    url: socket,
                    cipher: webCipher(parsed),
                    signHello: shareApi.viewerHello,
                });
                opened.start();
                setSession(opened);
            })
            .catch((error: unknown) => {
                if (live) setProblem(errorText(error));
            });
        return () => {
            live = false;
            opened?.stop();
        };
    }, [link]);

    if (!session)
        return (
            <div className="chat-connection-state stopped" role="status">
                <span>{problem ?? "Opening the shared chat…"}</span>
            </div>
        );
    return <SharedChat session={session} visible={visible} />;
}
