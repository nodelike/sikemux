import { useEffect, useRef, type RefObject } from "react";
import { useStore } from "../state/store";
import { AgentIcon, IconAgent, IconCommand, IconCopy, IconExternal, IconGlobe, IconRun, IconWindow } from "../ui/Icons";
import { ThemeIcon } from "../ui/FileIcon";
import { Tooltip } from "../ui/Tooltip";
import { copyPortUrl, openPortExternally, openPortOnDesk, revealPortOwner } from "./portActions";
import { processIcon } from "./processIcon";
import { deskAgentFor, type ProjectPort } from "./projectPorts";
import "../styles/ports-menu.css";

function useMenuKeys(menu: RefObject<HTMLDivElement | null>, close: () => void) {
    const closeRef = useRef(close);
    closeRef.current = close;
    useEffect(() => {
        const rows = () => [...(menu.current?.querySelectorAll<HTMLElement>(".tb-port-open") ?? [])];
        rows()[0]?.focus();
        const onKey = (event: KeyboardEvent) => {
            if (event.key === "Escape") {
                event.preventDefault();
                event.stopPropagation();
                closeRef.current();
                return;
            }
            if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
            const all = rows();
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

function ProcessIcon({ process }: { process: string }) {
    const icon = processIcon(process);
    if (!icon) return <IconGlobe size={13} />;
    return <ThemeIcon icon={icon} size={14} />;
}

function OwnerIcon({ owner }: { owner: ProjectPort["owner"] }) {
    const agentType = useStore((state) => (owner.reveal?.kind === "agent" ? state.agents[owner.reveal.agentId]?.type : undefined));
    if (agentType) return <AgentIcon type={agentType} size={11} className={`agent-glyph ${agentType}`} />;
    if (owner.kind === "task") return <IconRun size={11} />;
    if (owner.kind === "agent") return <IconAgent size={11} />;
    return <IconCommand size={11} />;
}

function PortRow({ port, deskAgent, deskTitle, close }: { port: ProjectPort; deskAgent: string | null; deskTitle: string; close: () => void }) {
    const act = (work: () => void) => () => {
        close();
        work();
    };
    const open = act(() => (deskAgent ? openPortOnDesk(deskAgent, port.url) : openPortExternally(port.url)));
    const { owner } = port;
    return (
        <div className="tb-port">
            <button
                className="tb-port-open"
                role="menuitem"
                onClick={open}
                onKeyDown={(event) => {
                    if (!event.metaKey) return;
                    if (event.key === "c") {
                        event.preventDefault();
                        act(() => copyPortUrl(port.url))();
                    } else if (event.key === "Enter" && deskAgent) {
                        event.preventDefault();
                        act(() => openPortExternally(port.url))();
                    }
                }}
                aria-label={deskAgent ? `Open localhost:${port.port} on ${deskTitle}'s desk` : `Open localhost:${port.port} in your browser`}>
                <span className="tb-port-lead" title={port.process || undefined}>
                    <ProcessIcon process={port.process} />
                </span>
                <span className="tb-port-addr">
                    :{port.port}
                    {port.preview && <span className="tb-port-preview" title="Preview" />}
                </span>
                <span className="tb-port-owner">
                    <span className="tb-port-owner-icon">
                        <OwnerIcon owner={owner} />
                    </span>
                    <span className="tb-port-owner-label">{owner.label}</span>
                </span>
            </button>
            <span className="tb-port-actions">
                {deskAgent && (
                    <Tooltip label="Open in your browser" side="left">
                        <button
                            className="tb-port-action"
                            role="menuitem"
                            tabIndex={-1}
                            aria-label="Open in your browser"
                            onClick={act(() => openPortExternally(port.url))}>
                            <IconExternal size={12} />
                        </button>
                    </Tooltip>
                )}
                <Tooltip label="Copy URL" side="left">
                    <button className="tb-port-action" role="menuitem" tabIndex={-1} aria-label="Copy URL" onClick={act(() => copyPortUrl(port.url))}>
                        <IconCopy size={12} />
                    </button>
                </Tooltip>
                {owner.reveal && (
                    <Tooltip label={`Show ${owner.label}`} side="left">
                        <button
                            className="tb-port-action"
                            role="menuitem"
                            tabIndex={-1}
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
                    {deskAgent ? (
                        <>
                            <IconAgent size={12} />
                            <span>Opens on</span>
                            <span className="tb-ports-desk">{deskTitle}</span>
                        </>
                    ) : (
                        <>
                            <IconGlobe size={12} />
                            <span>No agent running · opens in your browser</span>
                        </>
                    )}
                </div>
                <div className="tb-ports-rows">
                    {ports.map((port) => (
                        <PortRow key={port.port} port={port} deskAgent={deskAgent} deskTitle={deskTitle} close={close} />
                    ))}
                </div>
                <div className="tb-ports-keys" aria-hidden="true">
                    <span>
                        <kbd>↵</kbd> open
                    </span>
                    <span>
                        <kbd>⌘C</kbd> copy
                    </span>
                    {deskAgent && (
                        <span>
                            <kbd>⌘↵</kbd> browser
                        </span>
                    )}
                </div>
            </div>
        </>
    );
}
