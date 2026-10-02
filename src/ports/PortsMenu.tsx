import { useEffect, useRef, type RefObject } from "react";
import { useStore } from "../state/store";
import { IconAgent, IconCopy, IconExternal, IconWindow } from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";
import { copyPortUrl, openPortExternally, openPortOnDesk, revealPortOwner } from "./portActions";
import { deskAgentFor, type ProjectPort } from "./projectPorts";
import "../styles/ports-menu.css";

function useMenuKeys(menu: RefObject<HTMLDivElement | null>, close: () => void) {
    const closeRef = useRef(close);
    closeRef.current = close;
    useEffect(() => {
        const items = () => [...(menu.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];
        items()[0]?.focus();
        const onKey = (event: KeyboardEvent) => {
            if (event.key === "Escape") {
                event.preventDefault();
                event.stopPropagation();
                closeRef.current();
                return;
            }
            if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
            const all = items();
            if (!all.length) return;
            event.preventDefault();
            const at = all.indexOf(document.activeElement as HTMLElement);
            const step = event.key === "ArrowDown" ? 1 : -1;
            all[(at + step + all.length) % all.length]?.focus();
        };
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    }, [menu]);
}

function PortRow({ port, deskAgent, deskTitle, close }: { port: ProjectPort; deskAgent: string | null; deskTitle: string; close: () => void }) {
    const act = (work: () => void) => () => {
        close();
        work();
    };
    const { owner } = port;
    return (
        <div className="tb-port">
            <button
                className="tb-port-open"
                role="menuitem"
                onClick={act(() => (deskAgent ? openPortOnDesk(deskAgent, port.url) : openPortExternally(port.url)))}
                aria-label={deskAgent ? `Open localhost:${port.port} on ${deskTitle}'s desk` : `Open localhost:${port.port} in your browser`}>
                <span className="tb-port-line">
                    <span className="tb-port-addr">localhost:{port.port}</span>
                    {port.preview && <span className="tb-port-tag">preview</span>}
                </span>
                <span className="tb-port-meta">
                    {port.process || "process"} · {owner.label}
                </span>
            </button>
            <span className="tb-port-actions">
                {deskAgent && (
                    <Tooltip label="Open in your browser" side="left">
                        <button
                            className="tb-port-action"
                            role="menuitem"
                            aria-label="Open in your browser"
                            onClick={act(() => openPortExternally(port.url))}>
                            <IconExternal size={12} />
                        </button>
                    </Tooltip>
                )}
                <Tooltip label="Copy URL" side="left">
                    <button className="tb-port-action" role="menuitem" aria-label="Copy URL" onClick={act(() => copyPortUrl(port.url))}>
                        <IconCopy size={12} />
                    </button>
                </Tooltip>
                {owner.reveal && (
                    <Tooltip label={`Show ${owner.label}`} side="left">
                        <button
                            className="tb-port-action"
                            role="menuitem"
                            aria-label={`Show ${owner.label}`}
                            onClick={act(() => revealPortOwner(owner.reveal!))}>
                            {owner.kind === "agent" ? <IconAgent size={12} /> : <IconWindow size={12} />}
                        </button>
                    </Tooltip>
                )}
            </span>
        </div>
    );
}

export function PortsMenu({ sessionId, ports, close }: { sessionId: string; ports: ProjectPort[]; close: () => void }) {
    const menu = useRef<HTMLDivElement>(null);
    useMenuKeys(menu, close);
    const deskAgent = useStore((state) => deskAgentFor(state, sessionId));
    const deskTitle = useStore((state) => (deskAgent ? state.agents[deskAgent]?.title || "agent" : ""));
    return (
        <>
            <div className="env-dd-scrim" onClick={close} />
            <div className="env-dd-menu tb-ports-menu" role="menu" aria-label="Listening ports" ref={menu} data-overlay>
                <div className="tb-ports-head">
                    <span className="tb-ports-title">Listening</span>
                    <span className="tb-ports-hint">{deskAgent ? `opens on ${deskTitle}'s desk` : "no agent running · opens in your browser"}</span>
                </div>
                {ports.map((port) => (
                    <PortRow key={port.port} port={port} deskAgent={deskAgent} deskTitle={deskTitle} close={close} />
                ))}
            </div>
        </>
    );
}
