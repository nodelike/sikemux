import { copyWithNotice, runInTerminal } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { SkeletonRows, StaleNotice } from "../../../plugin-api/ui";
import type { Scope } from "../api";
import { powerTone, region, size } from "../format";
import { azureVmR } from "../resources";
import { Dot, Fact, Tags } from "./parts";
import { MachineActions } from "./Machines";

/** The ssh a person would type to reach a Linux machine on its public address. */
export function sshCommand(address: string, os: string): string[] | null {
    if (!address || os.toLowerCase() !== "linux") return null;
    return ["ssh", address];
}

export function MachineView({
    scope,
    group,
    name,
    active,
    onBack,
}: {
    scope: Scope;
    group: string;
    name: string;
    active: boolean;
    onBack: () => void;
}) {
    const machine = useResourceEnabled(active, azureVmR, { ...scope, group }, name);
    const found = machine.data;
    const ssh = found?.publicIp ? sshCommand(found.publicIp, found.os) : null;

    return (
        <section className="az-detail" aria-label={`Virtual machine ${name}`}>
            <header className="az-detail-head">
                <button type="button" className="az-back" onClick={onBack} aria-label="Back to virtual machines">
                    ‹ Virtual machines
                </button>
                <div className="az-detail-title">
                    {found ? <Dot tone={powerTone(found.power)} label={found.power} /> : null}
                    <h2 className="az-name">{name}</h2>
                    {found ? <span className={`az-state ${powerTone(found.power)}`}>{found.power}</span> : null}
                </div>
                <div className="az-detail-actions">
                    {ssh ? (
                        <button type="button" className="az-button" onClick={() => void runInTerminal(`ssh ${name}`, ssh)}>
                            Open SSH
                        </button>
                    ) : null}
                    {found ? <MachineActions machine={found} scope={scope} /> : null}
                </div>
            </header>
            {machine.status === "error" && !found ? (
                <p className="az-note az-error">{machine.error}</p>
            ) : !found ? (
                <SkeletonRows rows={6} label="Reading the machine" />
            ) : (
                <>
                    <StaleNotice handle={machine} />
                    <dl className="az-facts">
                        <Fact term="Size" value={size(found.size)} mono />
                        <Fact term="Operating system" value={found.os} />
                        <Fact term="Image" value={found.image} />
                        <Fact term="Region" value={region(found.location)} />
                        <Fact term="Zone" value={found.zones.join(", ") || "None"} />
                        <Fact term="Resource group" value={found.group} />
                        <Fact term="Private IP" value={found.privateIp} mono />
                        <Fact term="Public IP" value={found.publicIp} mono />
                    </dl>
                    <Tags tags={found.tags} />
                    <button type="button" className="az-link" onClick={() => copyWithNotice(found.id, "The resource id")}>
                        Copy the resource id
                    </button>
                </>
            )}
        </section>
    );
}
