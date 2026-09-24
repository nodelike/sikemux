import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type PointerEvent as ReactPointerEvent } from "react";
import { basename, confirmDialog } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, IconBruno, IconChevron } from "../../../plugin-api/ui";
import { mergeScope, type Scope } from "../lib/interpolate";
import { parseRequest } from "../lib/parse";
import { buildScope, findRequest, requestVars, selectedEnvOf } from "../lib/resolve";
import { runRequest, type RunResult } from "../lib/run";
import { serializeRequest } from "../lib/serialize";
import type { BruRequest, BruScope } from "../lib/types";
import { useBrunoDrafts, useBrunoSecretVars } from "../runtime";
import {
    DEFAULT_BRUNO_VIEW,
    brunoCollectionR,
    brunoSaveRequest,
    brunoSelectRequest,
    brunoSetDraft,
    brunoSetReqPanePct,
    brunoSetReqTab,
    brunoSetResTab,
    brunoSetSecret,
    brunoSettings,
    onRunRequested,
    openBrunoFolder,
    openPalette,
    rememberCollection,
    useBrunoView,
} from "../state";
import { BrunoEnvSelect } from "./BrunoEnvSelect";
import { BrunoTree } from "./BrunoTree";
import { BrunoRequestView } from "./BrunoRequest";
import { BrunoResponseView } from "./BrunoResponse";
import "../bruno.css";

interface Props {
    paneId: string;
    active: boolean;
}

function safeParse(text: string, fallback: BruRequest | null): BruRequest | null {
    try {
        return parseRequest(text);
    } catch {
        return fallback;
    }
}

export function BrunoPane({ paneId, active }: Props) {
    const collectionPath = brunoSettings.useSelect((settings) => settings.collectionPath);
    const selectedEnvs = brunoSettings.useSelect((settings) => settings.selectedEnvs);
    const knownWorkspaces = brunoSettings.useSelect((settings) => settings.workspaces.length);
    const view = useBrunoView(paneId);
    const drafts = useBrunoDrafts(paneId);
    const secretVars = useBrunoSecretVars(paneId);

    const coll = useResourceEnabled(active && !!collectionPath, brunoCollectionR, collectionPath);
    const collection = coll.data;
    useEffect(() => rememberCollection(collection ?? null), [collection]);

    const [results, setResults] = useState<Record<string, RunResult>>({});
    const [running, setRunning] = useState<Record<string, boolean>>({});
    const [editing, setEditing] = useState<{ path: string; req: BruRequest } | null>(null);
    const [runtime, setRuntime] = useState<Scope>({});
    const [trustedCollection, setTrustedCollection] = useState<string | null>(null);
    const splitRef = useRef<HTMLDivElement | null>(null);
    const reqPanePct = view.reqPanePct ?? DEFAULT_BRUNO_VIEW.reqPanePct;

    const path = view.activeRequestPath;
    const located = useMemo(() => (collection && path ? findRequest(collection.tree, path) : null), [collection, path]);
    const diskRequest = located?.request ?? null;
    const draft = path ? (drafts[path] ?? null) : null;

    const effectiveRequest = useMemo<BruRequest | null>(() => {
        if (editing && path && editing.path === path) return editing.req;
        if (draft != null) return safeParse(draft, diskRequest);
        return diskRequest;
    }, [editing, path, draft, diskRequest]);

    // Scope the environment picker to the open request's collection.
    const reqCollPath = located?.collectionPath ?? "";
    const visibleEnvs = useMemo(() => {
        if (!collection) return [];
        return reqCollPath ? collection.envs.filter((e) => e.collectionPath === reqCollPath) : collection.envs;
    }, [collection, reqCollPath]);
    const showEnvCollection = !reqCollPath; // only disambiguate by collection when not scoped
    const selectedEnvId = selectedEnvs[reqCollPath] ?? null;
    const env = collection ? selectedEnvOf(collection, selectedEnvId) : undefined;
    const secretNames = useMemo(() => env?.secretNames ?? [], [env?.secretNames]);
    const inheritedScope = useMemo(() => {
        if (!collection) return {} as Scope;
        return buildScope({ collection, env, secretVars, folderScopes: located?.folderScopes ?? [] });
    }, [collection, env, secretVars, located]);
    const scope = useMemo(() => mergeScope(runtime, requestVars(effectiveRequest), inheritedScope), [runtime, effectiveRequest, inheritedScope]);

    // Serialising the file's own version once per request, rather than once per
    // keystroke, to answer the only question asked of it: is this edited yet.
    const diskSerialized = useMemo(() => (diskRequest ? serializeRequest(diskRequest) : ""), [diskRequest]);

    const onChange = useCallback(
        (next: BruRequest) => {
            if (!path) return;
            setEditing({ path, req: next });
            const serialized = serializeRequest(next);
            brunoSetDraft(paneId, path, serialized === diskSerialized ? null : serialized);
        },
        [path, diskSerialized, paneId],
    );

    const onSend = useCallback(async () => {
        if (!path || !effectiveRequest || !collection) return;
        const scopes = [collection.config, ...(located?.folderScopes ?? [])].filter(Boolean) as BruScope[];
        let trusted = trustedCollection === collectionPath;
        if (!trusted) {
            trusted = await confirmDialog({
                title: "Trust this Bruno collection for this session?",
                body: "Trusted collections may run request scripts, contact localhost/private APIs, and upload files located inside the collection folder.\nRequests still have hard time and size limits.",
                confirmLabel: "Trust collection",
            });
            if (!trusted) return;
            setTrustedCollection(collectionPath);
        }
        setRunning((r) => ({ ...r, [path]: true }));
        try {
            const result = await runRequest({ request: effectiveRequest, scopes, vars: scope, trust: trusted, collectionPath });
            setResults((r) => ({ ...r, [path]: result }));
            if (Object.keys(result.envUpdates).length) {
                setRuntime((prev) => ({ ...prev, ...result.envUpdates }));
                // persist any script-updated secret values (e.g. a refreshed token)
                for (const [k, v] of Object.entries(result.envUpdates)) {
                    if (secretNames.includes(k)) brunoSetSecret(paneId, k, v);
                }
            }
        } finally {
            setRunning((r) => ({ ...r, [path]: false }));
        }
    }, [path, effectiveRequest, collection, located, scope, secretNames, paneId, trustedCollection, collectionPath]);

    const onSave = useCallback(() => {
        if (!path) return;
        void brunoSaveRequest(paneId, path).then(() => setEditing(null));
    }, [path, paneId]);

    const onSplitPointerDown = useCallback(
        (e: ReactPointerEvent<HTMLDivElement>) => {
            e.preventDefault();
            const handle = e.currentTarget;
            const el = splitRef.current;
            if (!el) return;
            const rect = el.getBoundingClientRect();
            if (rect.width <= 0) return;
            handle.setPointerCapture(e.pointerId);

            const minPx = 260;
            const minPct = Math.min(45, (minPx / rect.width) * 100);
            const maxPct = Math.max(55, 100 - minPct);

            const move = (ev: PointerEvent) => {
                const raw = ((ev.clientX - rect.left) / rect.width) * 100;
                const next = Math.max(minPct, Math.min(maxPct, raw));
                brunoSetReqPanePct(paneId, Math.round(next * 10) / 10);
            };
            const up = () => {
                document.body.classList.remove("bruno-resizing");
                handle.removeEventListener("pointermove", move);
                handle.removeEventListener("pointerup", up);
            };

            document.body.classList.add("bruno-resizing");
            handle.addEventListener("pointermove", move);
            handle.addEventListener("pointerup", up);
        },
        [paneId],
    );

    const onSplitKeyDown = useCallback(
        (event: React.KeyboardEvent<HTMLDivElement>) => {
            if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
            event.preventDefault();
            const step = event.shiftKey ? 5 : 2;
            const next = Math.max(20, Math.min(80, reqPanePct + (event.key === "ArrowRight" ? step : -step)));
            brunoSetReqPanePct(paneId, next);
        },
        [reqPanePct, paneId],
    );

    useEffect(() => () => document.body.classList.remove("bruno-resizing"), []);

    useEffect(
        () =>
            onRunRequested((requested) => {
                if (requested === paneId) void onSend();
            }),
        [paneId, onSend],
    );

    if (!collectionPath) {
        return (
            <div className="bruno-pane bruno-empty">
                <EmptyState
                    icon={<IconBruno size={20} />}
                    title="No workspace loaded"
                    message="Load a Bruno collection folder to browse and run its requests."
                    action={
                        knownWorkspaces > 0
                            ? { label: "Choose workspace", onClick: () => openPalette("workspacePalette") }
                            : { label: "Add workspace", onClick: () => void openBrunoFolder() }
                    }
                />
            </div>
        );
    }

    return (
        <div className="bruno-pane" data-active={active ? "1" : "0"}>
            <header className="bruno-head">
                <span className="bruno-head-mark">
                    <IconBruno size={15} />
                </span>
                <button type="button" className="dd-btn bruno-workspace-dd" title={collectionPath} onClick={() => openPalette("workspacePalette")}>
                    <span className="dd-val bruno-coll-name">{collection?.name || basename(collectionPath)}</span>
                    <IconChevron size={9} className="dd-chev" />
                </button>
                <BrunoEnvSelect
                    paneId={paneId}
                    envs={visibleEnvs}
                    showCollection={showEnvCollection}
                    selected={selectedEnvId}
                    secretNames={secretNames}
                    secretVars={secretVars}
                    secretsOpen={view.secretsOpen}
                />
            </header>
            <div className="bruno-cols">
                <BrunoTree
                    paneId={paneId}
                    collectionPath={collectionPath}
                    tree={collection?.tree ?? []}
                    activePath={path}
                    drafts={drafts}
                    running={running}
                    loading={coll.status === "loading" && !collection}
                    error={coll.error ?? null}
                    onSelect={(p) => brunoSelectRequest(paneId, p)}
                    onReload={() => void coll.refresh()}
                />
                <div className="bruno-main">
                    {effectiveRequest && path ? (
                        <div
                            className="bruno-workbench"
                            data-document-host
                            ref={splitRef}
                            style={{ "--bruno-req-pct": `${reqPanePct}%` } as CSSProperties}>
                            <BrunoRequestView
                                request={effectiveRequest}
                                tab={view.reqTab}
                                scope={scope}
                                running={!!running[path]}
                                dirty={draft != null}
                                onChange={onChange}
                                onSend={() => void onSend()}
                                onSave={onSave}
                                onTab={(t) => brunoSetReqTab(paneId, t)}
                            />
                            <div
                                className="bruno-splitter"
                                role="separator"
                                tabIndex={0}
                                aria-orientation="vertical"
                                aria-valuemin={20}
                                aria-valuemax={80}
                                aria-valuenow={Math.round(reqPanePct)}
                                title="Drag or use arrow keys to resize"
                                onPointerDown={onSplitPointerDown}
                                onKeyDown={onSplitKeyDown}
                            />
                            <BrunoResponseView
                                result={results[path] ?? null}
                                running={!!running[path]}
                                tab={view.resTab}
                                onTab={(t) => brunoSetResTab(paneId, t)}
                            />
                        </div>
                    ) : (
                        <div className="bruno-empty bruno-empty-main">
                            <span>select a request from the collection</span>
                        </div>
                    )}
                </div>
            </div>
        </div>
    );
}
