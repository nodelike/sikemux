import { lazy, Suspense, useEffect, useState } from "react";
import { IconGlobe } from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";
import { useProjectPorts } from "./useProjectPorts";

const PortsMenu = lazy(() => import("./PortsMenu").then((module) => ({ default: module.PortsMenu })));

export function PortsChip({ sessionId }: { sessionId: string }) {
    const ports = useProjectPorts(sessionId);
    const [open, setOpen] = useState(false);
    const empty = ports.length === 0;
    useEffect(() => {
        if (empty) setOpen(false);
    }, [empty]);
    if (empty) return null;
    const label = ports.length === 1 ? "1 listening port" : `${ports.length} listening ports`;
    return (
        <>
            <span className="tb-ports" data-no-window-drag>
                <Tooltip label={label} disabled={open}>
                    <button
                        className="tb-ports-chip"
                        onClick={() => setOpen((value) => !value)}
                        aria-haspopup="menu"
                        aria-expanded={open}
                        aria-label={label}>
                        <IconGlobe size={12} />
                        <span className="tb-ports-count">{ports.length}</span>
                    </button>
                </Tooltip>
                {open && (
                    <Suspense fallback={null}>
                        <PortsMenu sessionId={sessionId} ports={ports} close={() => setOpen(false)} />
                    </Suspense>
                )}
            </span>
            <span className="tb-sep" />
        </>
    );
}
