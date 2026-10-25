import { useState } from "react";
import type { AccountStatus } from "../api/account";
import { initials } from "@shared/lib/initials";

/** The account's picture, or its initials when it has none or the picture does not load. */
export function AccountAvatar({ account, className }: { account: AccountStatus; className: string }) {
    const [broken, setBroken] = useState<string | null>(null);
    const picture = account.picture !== broken ? account.picture : null;
    return picture ? (
        <img className={className} src={picture} alt="" draggable={false} onError={() => setBroken(picture)} />
    ) : (
        <span className={className} aria-hidden="true">
            {initials(account.name, account.email)}
        </span>
    );
}
