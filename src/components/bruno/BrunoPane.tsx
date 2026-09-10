import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type PointerEvent as ReactPointerEvent } from "react";
import * as cmd from "../../state/commands";
import { subscribe } from "../../state/bus";
import { useResourceEnabled } from "../../state/resources";
import { brunoCollectionR } from "../../state/resources.defs";
import { useStore } from "../../state/store";
import { DEFAULT_BRUNO_VIEW } from "../../state/types";
import { parseRequest } from "../../bruno/parse";
import { serializeRequest } from "../../bruno/serialize";
import { buildScope, findRequest, requestVars, selectedEnvOf } from "../../bruno/resolve";
import { mergeScope, type Scope } from "../../bruno/interpolate";
import { runRequest, type RunResult } from "../../bruno/run";
import type { BruRequest, BruScope } from "../../bruno/types";
import { basename, relativePath as pathRelative } from "../../lib/paths";
import { FILE_MANAGER_NAME } from "../../lib/platform";
import { fsapi } from "../../api/fs";
import { notify, reportError } from "../../state/toast";
import { confirmDialog } from "../../state/dialog";
import { type CtxItem } from "../FileTree";
import { TabBar } from "../TabBar";
import { IconBruno } from "../Icons";
import { BrunoEnvSelect } from "./BrunoEnvSelect";
import { BrunoTree } from "./BrunoTree";
import { BrunoRequestView } from "./BrunoRequest";
import { BrunoResponseView } from "./BrunoResponse";

interface Props {
    paneId: string;
    sessionId: string;
    active: boolean;
}

function safeParse(text: string, fallback: BruRequest | null): BruRequest | null {
    try {
        return parseRequest(text);
    } catch {
        return fallback;
    }
}

export function BrunoPane({ sessionId, active }: Props) {
    const session = useStore((s) => s.sessions[sessionId]);
    const bruno = session?.bruno ?? null;
    const collectionPath = bruno?.collectionPath ?? "";
    const view = useStore((s) => s.brunoViews[sessionId] ?? DEFAULT_BRUNO_VIEW);
    const drafts = useMemo(() => bruno?.drafts ?? {}, [bruno?.drafts]);
    const secretVars = useMemo(() => bruno?.secretVars ?? {}, [bruno?.secretVars]);
    const selectedEnvs = bruno?.selectedEnvs ?? {};

    const coll = useResourceEnabled(active && !!collectionPath, brunoCollectionR, collectionPath);
    const collection = coll.data;

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

    const openTabs = useMemo(
        () =>
            view.openPaths.map((p) => {
                const loc = collection ? findRequest(collection.tree, p) : null;
                return {
                    path: p,
                    name: loc?.request.meta.name || basename(p).replace(/\.bru$/, ""),
                    method: loc?.request.method ?? "get",
                    dirty: drafts[p] != null,
                };
            }),
        [view.openPaths, collection, drafts],
    );

    const onChange = useCallback(
        (next: BruRequest) => {
            if (!path) return;
            setEditing({ path, req: next });
            const serialized = serializeRequest(next);
            const diskSer = diskRequest ? serializeRequest(diskRequest) : "";
            cmd.brunoSetDraft(sessionId, path, serialized === diskSer ? null : serialized);
        },
        [path, diskRequest, sessionId],
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
                    if (secretNames.includes(k)) cmd.brunoSetSecret(sessionId, k, v);
                }
            }
        } finally {
            setRunning((r) => ({ ...r, [path]: false }));
        }
    }, [path, effectiveRequest, collection, located, scope, secretNames, sessionId, trustedCollection, collectionPath]);

    const onSave = useCallback(() => {
        if (!path) return;
        void cmd.brunoSaveRequest(sessionId, path).then(() => setEditing(null));
    }, [path, sessionId]);

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
                cmd.brunoSetReqPanePct(sessionId, Math.round(next * 10) / 10);
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
        [sessionId],
    );

    const onSplitKeyDown = useCallback(
        (event: React.KeyboardEvent<HTMLDivElement>) => {
            if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
            event.preventDefault();
            const step = event.shiftKey ? 5 : 2;
            const next = Math.max(20, Math.min(80, reqPanePct + (event.key === "ArrowRight" ? step : -step)));
            cmd.brunoSetReqPanePct(sessionId, next);
        },
        [reqPanePct, sessionId],
    );

    useEffect(() => () => document.body.classList.remove("bruno-resizing"), []);

    // ⌘↵ from anywhere in the pane: the keymap emits, we run.
    useEffect(() => {
        return subscribe("bruno-run", (e) => {
            if (e.sessionId === sessionId) void onSend();
        });
    }, [sessionId, onSend]);

    const relativePath = (p: string) => pathRelative(p, collectionPath) ?? basename(p);
    const copyText = (text: string, label: string) =>
        navigator.clipboard.writeText(text).then(() => notify("success", `copied ${label}`), reportError("copy"));

    const buildTabMenu = (tabPath: string): CtxItem[] => {
        const open = view.openPaths;
        const idx = open.indexOf(tabPath);
        const others = open.filter((p) => p !== tabPath);
        const toLeft = open.slice(0, idx);
        const toRight = open.slice(idx + 1);
        return [
            { label: "Close", hint: "⌥W", run: () => cmd.brunoCloseTab(sessionId, tabPath) },
            { label: "Close Others", disabled: others.length === 0, run: () => others.forEach((p) => cmd.brunoCloseTab(sessionId, p)) },
            { label: "Close to the Left", disabled: toLeft.length === 0, run: () => toLeft.forEach((p) => cmd.brunoCloseTab(sessionId, p)) },
            { label: "Close to the Right", disabled: toRight.length === 0, run: () => toRight.forEach((p) => cmd.brunoCloseTab(sessionId, p)) },
            { label: "Close All", run: () => open.forEach((p) => cmd.brunoCloseTab(sessionId, p)) },
            { sep: true },
            { label: "Copy Path", run: () => void copyText(tabPath, "path") },
            { label: "Copy Relative Path", run: () => void copyText(relativePath(tabPath), "relative path") },
            { sep: true },
            { label: `Reveal in ${FILE_MANAGER_NAME}`, run: () => void fsapi.revealInFinder(tabPath).catch(reportError("reveal")) },
        ];
    };

    if (!bruno) return <div className="bruno-pane bruno-empty">not a Bruno workspace</div>;

    return (
        <div className="bruno-pane" data-active={active ? "1" : "0"}>
            <header className="bruno-head">
                <span className="bruno-head-mark">
                    <IconBruno size={15} />
                </span>
                <span className="bruno-coll-name" title={collectionPath}>
                    {collection?.name || basename(collectionPath)}
                </span>
                <BrunoEnvSelect
                    sessionId={sessionId}
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
                    sessionId={sessionId}
                    collectionPath={collectionPath}
                    tree={collection?.tree ?? []}
                    activePath={path}
                    drafts={drafts}
                    running={running}
                    loading={coll.status === "loading" && !collection}
                    error={coll.error ?? null}
                    onSelect={(p) => cmd.brunoSelectRequest(sessionId, p)}
                    onReload={() => void coll.refresh()}
                />
                <div className="bruno-main">
                    {openTabs.length > 0 && (
                        <TabBar
                            variant="bruno"
                            tabs={openTabs.map((t) => ({
                                id: t.path,
                                tabId: `bruno-tab-${sessionId}-${encodeURIComponent(t.path)}`,
                                panelId: `bruno-content-${sessionId}`,
                                label: t.name,
                                title: t.path,
                                active: t.path === path,
                                dirty: t.dirty,
                                icon: <span className={`bruno-method m-${t.method}`}>{t.method.toUpperCase()}</span>,
                            }))}
                            onSelect={(p) => cmd.brunoSelectRequest(sessionId, p)}
                            onClose={(p) => cmd.brunoCloseTab(sessionId, p)}
                            buildMenu={buildTabMenu}
                        />
                    )}
                    {effectiveRequest && path ? (
                        <div
                            id={`bruno-content-${sessionId}`}
                            role="tabpanel"
                            aria-labelledby={`bruno-tab-${sessionId}-${encodeURIComponent(path)}`}
                            className="bruno-workbench"
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
                                onTab={(t) => cmd.brunoSetReqTab(sessionId, t)}
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
                                onTab={(t) => cmd.brunoSetResTab(sessionId, t)}
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
