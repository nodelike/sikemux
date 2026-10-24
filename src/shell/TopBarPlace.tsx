import { useMemo } from "react";
import { useShallow } from "zustand/react/shallow";
import { chooseHost, setUpFor, setUpOverSsh, SSH_WORK, sshSetUp, THIS_HOST, type HostChoice } from "../hosts/hosts";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { Dropdown, type DropdownOption } from "../ui/Dropdown";
import { IconChevron, IconDownload, IconLogIn, IconPower } from "../ui/Icons";
import { hostMark } from "../hosts/placeMarks";
import { ProjectPicker } from "../projects/ProjectPicker";
import { LEAVES_SETTINGS } from "../settings/leaveSettings";

const keyOf = (choice: HostChoice) => (choice.kind === "core" ? `core:${choice.host ?? ""}` : `ssh:${choice.alias}`);

function choiceOf(key: string): HostChoice {
    const [kind, rest] = [key.slice(0, key.indexOf(":")), key.slice(key.indexOf(":") + 1)];
    return kind === "ssh" ? { kind: "ssh", alias: rest } : rest ? { kind: "core", host: rest } : THIS_HOST;
}

/** Where an SSH host without a running Sikemux has no projects: the one thing that sets it up (ADR 0019). */
function SetUpOffer({ alias }: { alias: string }) {
    const setUp = useStore((s) => sshSetUp(s, alias));
    const work = useStore((s) => s.sshWork[alias]);
    if (!setUp) return null;
    const { offer, running } = SSH_WORK[work ?? setUp];
    const label = work
        ? `${running[0].toUpperCase()}${running.slice(1)} ${alias}…`
        : setUp === "signing in"
          ? `Sign in ${alias}`
          : `${offer} on ${alias}`;
    const icon = setUp === "signing in" ? <IconLogIn size={13} /> : setUp === "starting" ? <IconPower size={13} /> : <IconDownload size={13} />;
    return (
        <>
            <span className="tb-place-sep" aria-hidden="true">
                <IconChevron size={11} />
            </span>
            <button type="button" className="dd-btn tb-place-btn" disabled={!!work} onClick={() => setUpOverSsh(alias, setUp)}>
                <span className="dd-icon" aria-hidden="true">
                    {icon}
                </span>
                <span className="dd-val">{label}</span>
            </button>
        </>
    );
}

/** Where the window is: the chosen host, then the project in front, each a picker, in the middle of the top bar. */
export function TopBarPlace() {
    const chosen = useStore((s) => s.chosenHost);
    const hosts = useStore((s) => s.hosts);
    const sshAliases = useStore(
        useShallow((s) => [...new Set(s.sessionOrder.flatMap((id) => (s.sessions[id]?.kind === "ssh" ? [s.sessions[id].name] : [])))]),
    );
    const activeProject = useStore((s) => {
        const session = s.sessions[s.activeSessionId];
        const onHost = session?.kind === "project" && s.chosenHost.kind === "core" && session.host === s.chosenHost.host;
        return onHost ? session.cwd : null;
    });
    // An SSH host has only its set-up to offer; a host on the account offers it while its Sikemux needs it.
    const setUpAlias = useStore((s) => {
        const host = s.chosenHost;
        if (host.kind === "ssh") return host.alias;
        const row = s.hosts.find((candidate) => candidate.id === host.host);
        return row?.ssh && setUpFor(row) ? row.ssh : null;
    });

    const hostOptions = useMemo<DropdownOption[]>(() => {
        const installed = new Set(hosts.flatMap((host) => host.ssh ?? []));
        return [
            { value: keyOf(THIS_HOST), label: "This host", icon: hostMark(THIS_HOST) },
            ...hosts.map((host) => ({
                value: keyOf({ kind: "core", host: host.id }),
                label: host.name,
                icon: hostMark({ kind: "core", host: host.id }),
            })),
            ...sshAliases
                .filter((alias) => !installed.has(alias))
                .map((alias) => ({ value: keyOf({ kind: "ssh", alias }), label: alias, icon: hostMark({ kind: "ssh", alias }) })),
        ];
    }, [hosts, sshAliases]);
    const hostKey = keyOf(chosen);

    return (
        <div className="tb-place" data-no-window-drag {...LEAVES_SETTINGS}>
            <Dropdown
                className="tb-place-btn"
                label="Host"
                value={hostKey}
                options={hostOptions}
                icon={hostMark(chosen)}
                onChange={(key) => chooseHost(choiceOf(key))}
            />
            {setUpAlias ? (
                <SetUpOffer alias={setUpAlias} />
            ) : (
                chosen.kind === "core" &&
                activeProject !== null && (
                    <>
                        <span className="tb-place-sep" aria-hidden="true">
                            <IconChevron size={11} />
                        </span>
                        <ProjectPicker
                            className="tb-place-btn"
                            host={chosen.host}
                            value={activeProject}
                            onPick={(choice) => (choice.sessionId ? cmd.selectSession(choice.sessionId) : cmd.createProjectSession(choice.path))}
                        />
                    </>
                )
            )}
        </div>
    );
}
