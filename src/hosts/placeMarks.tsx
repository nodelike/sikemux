import { IconFolder, IconLaptop, IconServer } from "@shared/ui/Icons";
import type { HostChoice } from "./hosts";

/** A host's mark in the side rail's colour for its kind: this host, one on the account, or an SSH host. */
export function hostMark(choice: HostChoice, size = 13) {
    if (choice.kind === "core" && !choice.host) return <IconLaptop size={size} />;
    return (
        <span className={`place-mark ${choice.kind === "core" ? "host-sikemux" : "host-ssh"}`}>
            <IconServer size={size} />
        </span>
    );
}

export function projectMark(size = 13) {
    return (
        <span className="place-mark project">
            <IconFolder size={size} />
        </span>
    );
}
