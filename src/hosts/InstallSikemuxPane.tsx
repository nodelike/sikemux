import { setUpOverSsh, SSH_WORK, sshSetUp, type SshWork } from "./hosts";
import { useStore } from "../state/store";
import { IconDownload, IconLogIn, IconPower } from "@shared/ui/Icons";
import { EmptyState } from "../ui/Panel";

/** What each way of setting an SSH host up does there, said before the person does it. */
function what(work: SshWork, alias: string): { title: string; message: string } {
    switch (work) {
        case "installing":
            return {
                title: `Install Sikemux on ${alias}`,
                message: `With Sikemux, ${alias} runs agents that keep working while you are away, your phone reaches it, and its projects, chats and files open here without SSH. The installer runs over your SSH in a new tab, where you type sudo's password if it asks.`,
            };
        case "signing in":
            return {
                title: `Sign in ${alias}`,
                message: `Sikemux is on ${alias} but not signed in. Signing in runs sikemux login there, in a new tab, and adds it to your account.`,
            };
        case "starting":
            return {
                title: `Start Sikemux on ${alias}`,
                message: `Sikemux is on ${alias} but its core is stopped. Starting runs sikemux start there.`,
            };
        case "updating":
            return {
                title: `Update Sikemux on ${alias}`,
                message: `${alias} runs a release of Sikemux too old for this app. Updating runs the installer over your SSH in a new tab, then restarts its core.`,
            };
    }
}

/** What setting up an SSH host adds, and the one action that does it (ADR 0019). */
export function InstallSikemuxPane({ alias }: { alias: string }) {
    const setUp = useStore((s) => sshSetUp(s, alias));
    const work = useStore((s) => s.sshWork[alias]);
    if (!setUp) {
        return (
            <div className="install-sikemux-pane">
                <EmptyState icon={<IconDownload size={22} />} message={`${alias} runs Sikemux, on your account or another one.`} />
            </div>
        );
    }
    const { title, message } = what(setUp, alias);
    const icon = setUp === "signing in" ? <IconLogIn size={22} /> : setUp === "starting" ? <IconPower size={22} /> : <IconDownload size={22} />;
    return (
        <div className="install-sikemux-pane">
            <EmptyState
                icon={icon}
                title={title}
                message={
                    work
                        ? `${SSH_WORK[work].running[0].toUpperCase()}${SSH_WORK[work].running.slice(1)}… a password, if ${alias} asks for one, goes in the tab beside this one.`
                        : message
                }
                action={work ? undefined : { label: SSH_WORK[setUp].offer, onClick: () => setUpOverSsh(alias, setUp) }}
            />
        </div>
    );
}
