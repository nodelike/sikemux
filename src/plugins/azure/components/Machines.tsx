import { useState } from "react";
import { confirmDialog, notify } from "../../../plugin-api/host";
import { errorText } from "../../../plugin-api/format";
import { azure } from "../resources";
import { azureApi, type Machine, type MachineAction, type Scope } from "../api";
import { actionsFor, powerTone, region, size } from "../format";
import { Dot } from "./parts";

/** What stopping a machine is worth saying first, since deallocating takes its address away. */
export function warningFor(action: MachineAction, name: string): { title: string; body: string } | null {
    if (action === "deallocate") {
        return {
            title: `Stop and deallocate ${name}?`,
            body: "It stops costing compute, but it gives up its memory, and a public IP that isn't static is handed back.",
        };
    }
    if (action === "powerOff") {
        return { title: `Stop ${name}?`, body: "The operating system shuts down, but the machine stays allocated and keeps costing." };
    }
    if (action === "restart") {
        return { title: `Restart ${name}?`, body: "Anything running on it stops while it comes back." };
    }
    return null;
}

export function Machines({
    machines,
    scope,
    showGroup,
    onOpen,
}: {
    machines: Machine[];
    scope: Scope;
    showGroup: boolean;
    onOpen: (machine: Machine) => void;
}) {
    if (machines.length === 0) return <p className="az-note">No virtual machines here.</p>;
    return (
        <section className="az-section" aria-label="Virtual machines">
            <table className="az-table" aria-label="Virtual machines">
                <thead>
                    <tr>
                        <th aria-label="Power" />
                        <th>Name</th>
                        <th>Power</th>
                        <th>Size</th>
                        <th>OS</th>
                        {showGroup ? <th>Resource group</th> : null}
                        <th>Region</th>
                        <th aria-label="Actions" />
                    </tr>
                </thead>
                <tbody>
                    {machines.map((machine) => (
                        <tr
                            key={machine.id}
                            className="az-row"
                            tabIndex={0}
                            aria-label={machine.name}
                            onClick={() => onOpen(machine)}
                            onKeyDown={(event) => {
                                if (event.key === "Enter") onOpen(machine);
                            }}>
                            <td className="az-cell-dot">
                                <Dot tone={powerTone(machine.power)} label={machine.power} />
                            </td>
                            <td className="az-name">{machine.name}</td>
                            <td className={`az-state ${powerTone(machine.power)}`}>{machine.power}</td>
                            <td className="az-mono">{size(machine.size)}</td>
                            <td className="az-muted">{machine.os}</td>
                            {showGroup ? <td className="az-muted">{machine.group}</td> : null}
                            <td className="az-muted">{region(machine.location)}</td>
                            <td className="az-cell-actions">
                                <MachineActions machine={machine} scope={scope} />
                            </td>
                        </tr>
                    ))}
                </tbody>
            </table>
        </section>
    );
}

export function MachineActions({ machine, scope }: { machine: Machine; scope: Scope }) {
    const [busy, setBusy] = useState<MachineAction | null>(null);
    const run = async (action: MachineAction, label: string) => {
        const warning = warningFor(action, machine.name);
        if (warning) {
            const yes = await confirmDialog({
                title: warning.title,
                body: warning.body,
                confirmLabel: label,
                destructive: action === "deallocate",
            });
            if (!yes) return;
        }
        setBusy(action);
        try {
            const done = await azureApi.vmAct({ ...scope, group: machine.group }, machine.name, action);
            notify("success", done.message);
            azure.refresh();
        } catch (failure) {
            notify("error", errorText(failure));
        } finally {
            setBusy(null);
        }
    };
    return (
        <span className="az-row-actions">
            {actionsFor(machine).map((each) => (
                <button
                    key={each.action}
                    type="button"
                    className={`az-button small${each.danger ? " danger" : ""}`}
                    disabled={busy !== null}
                    aria-label={`${each.label} ${machine.name}`}
                    onClick={(event) => {
                        event.stopPropagation();
                        void run(each.action, each.label);
                    }}
                    onKeyDown={(event) => event.stopPropagation()}>
                    {busy === each.action ? "…" : each.label}
                </button>
            ))}
        </span>
    );
}
