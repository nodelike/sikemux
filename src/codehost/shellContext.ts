import { createContext, useContext, type ReactNode } from "react";

/** What the host shell around the git pane's local screens lends them. */
export interface GitShell {
    /** Opens the picker for which host repository the pane shows, while there is one to pick. */
    pickRepo: (() => void) | null;
    /** Who is signed in to the host, with the menu to switch accounts or repositories. */
    account: ReactNode;
    /** Whether the host has an inbox for the signed-in account. */
    inbox: boolean;
}

export const GitShellContext = createContext<GitShell>({ pickRepo: null, account: null, inbox: false });

export const useGitShell = () => useContext(GitShellContext);
