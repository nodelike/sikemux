import { IconPlus, IconRun, IconSave, IconTrash, PRIMARY_SHORTCUT } from "../../../plugin-api/ui";
import { interpolate, type Scope } from "../lib/interpolate";
import { HTTP_METHODS, type AuthMode, type BodyMode, type BruRequest, type KeyVal } from "../lib/types";
import type { BrunoReqTab } from "../state";
import { VarInput } from "./VarText";
import { BrunoSelect, BrunoCheck, type BrunoOption } from "./BrunoControls";
import { BrunoCode, type BrunoLang } from "./BrunoCode";

interface Props {
    request: BruRequest;
    tab: BrunoReqTab;
    scope: Scope;
    running: boolean;
    dirty: boolean;
    onChange: (req: BruRequest) => void;
    onSend: () => void;
    onSave: () => void;
    onTab: (t: BrunoReqTab) => void;
}

const REQ_TABS: BrunoReqTab[] = ["params", "body", "headers", "auth", "vars", "script", "docs"];
const BODY_MODES: BodyMode[] = ["none", "json", "text", "xml", "graphql", "sparql", "form-urlencoded", "multipart-form", "file"];
const AUTH_MODES: AuthMode[] = ["inherit", "none", "bearer", "basic", "apikey"];
const TEXT_MODES = new Set<BodyMode>(["json", "text", "xml", "graphql", "sparql"]);
const bodyLang = (mode: BodyMode): BrunoLang => (mode === "json" || mode === "graphql" ? "json" : mode === "xml" ? "xml" : "text");

const METHOD_OPTS: BrunoOption[] = HTTP_METHODS.map((m) => ({ value: m, label: m.toUpperCase(), className: `m-${m}` }));
const BODY_OPTS: BrunoOption[] = BODY_MODES.map((m) => ({ value: m, label: m }));
const AUTH_OPTS: BrunoOption[] = AUTH_MODES.map((m) => ({ value: m, label: m }));
const APIKEY_PLACEMENT_OPTS: BrunoOption[] = [
    { value: "header", label: "Header" },
    { value: "queryparams", label: "Query param" },
];

function KeyValTable({
    rows,
    scope,
    onChange,
    namePh = "name",
    valuePh = "value",
}: {
    rows: KeyVal[];
    scope: Scope;
    onChange: (r: KeyVal[]) => void;
    namePh?: string;
    valuePh?: string;
}) {
    const display = [...rows, { name: "", value: "", enabled: true }];
    const commit = (i: number, patch: Partial<KeyVal>) => {
        if (i < rows.length) onChange(rows.map((r, idx) => (idx === i ? { ...r, ...patch } : r)));
        else onChange([...rows, { name: "", value: "", enabled: true, ...patch }]);
    };
    const remove = (i: number) => onChange(rows.filter((_, idx) => idx !== i));
    return (
        <div className="bruno-kv">
            {display.map((r, i) => {
                const real = i < rows.length;
                return (
                    <div className={`bruno-kv-row${real ? "" : " add"}`} key={i}>
                        {real ? (
                            <BrunoCheck
                                checked={r.enabled}
                                onChange={(enabled) => commit(i, { enabled })}
                                title={r.enabled ? "Disable row" : "Enable row"}
                            />
                        ) : (
                            <span className="bruno-check ghost" />
                        )}
                        <VarInput value={r.name} scope={scope} placeholder={namePh} onChange={(v) => commit(i, { name: v })} />
                        <VarInput value={r.value} scope={scope} placeholder={valuePh} onChange={(v) => commit(i, { value: v })} />
                        {real ? (
                            <button className="bruno-kv-del" title="Remove" onClick={() => remove(i)}>
                                <IconTrash size={13} />
                            </button>
                        ) : (
                            <span className="bruno-kv-del ghost">
                                <IconPlus size={12} />
                            </span>
                        )}
                    </div>
                );
            })}
        </div>
    );
}

function count(rows: KeyVal[]): number {
    return rows.filter((r) => r.enabled && r.name).length;
}

export function BrunoRequestView({ request, tab, scope, running, dirty, onChange, onSend, onSave, onTab }: Props) {
    const set = (patch: Partial<BruRequest>) => onChange({ ...request, ...patch });
    const setBody = (patch: Partial<BruRequest["body"]>) => onChange({ ...request, body: { ...request.body, ...patch } });
    const setAuth = (patch: Partial<BruRequest["auth"]>) => onChange({ ...request, auth: { ...request.auth, ...patch } });
    const resolvedUrl = interpolate(request.url, scope);

    const badge = (t: BrunoReqTab): { n?: number; dot?: boolean } => {
        if (t === "headers") return { n: count(request.headers) || undefined };
        if (t === "params") return { n: count(request.params.query) || undefined };
        if (t === "vars") return { n: count(request.vars.req) + count(request.vars.res) || undefined };
        if (t === "body") return { dot: request.body.mode !== "none" };
        if (t === "auth") return { dot: request.auth.mode !== "none" && request.auth.mode !== "inherit" };
        if (t === "docs") return { dot: !!request.docs };
        if (t === "script") return { dot: !!(request.scripts.pre || request.scripts.post) };
        return {};
    };

    return (
        <div className="bruno-request">
            <div className="bruno-urlbar">
                <BrunoSelect
                    value={request.method}
                    options={METHOD_OPTS}
                    onChange={(m) => set({ method: m as BruRequest["method"] })}
                    className="bruno-method-dd"
                    title="Method"
                    menuWidth={132}
                />
                <VarInput
                    className="bruno-url"
                    value={request.url}
                    scope={scope}
                    placeholder="https://… or {{base_url}}/path"
                    onChange={(v) => set({ url: v })}
                    onKeyDown={(e) => {
                        if (e.key === "Enter") onSend();
                    }}
                />
                <button className="bruno-send" onClick={onSend} disabled={running} title={`Send (${PRIMARY_SHORTCUT}↵)`}>
                    {running ? <span className="bruno-row-spin" /> : <IconRun size={12} />}
                    <span>{running ? "Sending" : "Send"}</span>
                    {!running && (
                        <kbd className="bruno-send-kbd">
                            <span className="bruno-kbd-cmd">{PRIMARY_SHORTCUT}</span>
                            <span className="bruno-kbd-ret">↵</span>
                        </kbd>
                    )}
                </button>
                <button
                    className={`bruno-save${dirty ? " dirty" : ""}`}
                    onClick={onSave}
                    disabled={!dirty}
                    title={`Save to .bru (${PRIMARY_SHORTCUT}S)`}>
                    <IconSave size={13} />
                </button>
            </div>
            {request.url.includes("{{") && (
                <div className="bruno-url-preview" title={resolvedUrl}>
                    <span className="bruno-url-preview-tag">→</span>
                    {resolvedUrl}
                </div>
            )}

            <div className="bruno-tabs">
                {REQ_TABS.map((t) => {
                    const b = badge(t);
                    return (
                        <button key={t} className={`bruno-tab${tab === t ? " active" : ""}`} onClick={() => onTab(t)}>
                            {t}
                            {b.n != null && <span className="bruno-tab-count">{b.n}</span>}
                            {b.dot && <span className="bruno-tab-dot" />}
                        </button>
                    );
                })}
            </div>

            <div className="bruno-tab-body">
                {tab === "params" && (
                    <div className="bruno-section">
                        <div className="panel-label">Query params</div>
                        <KeyValTable rows={request.params.query} scope={scope} onChange={(query) => set({ params: { ...request.params, query } })} />
                        {request.params.path.length > 0 && (
                            <>
                                <div className="panel-label">Path params</div>
                                <KeyValTable
                                    rows={request.params.path}
                                    scope={scope}
                                    onChange={(path) => set({ params: { ...request.params, path } })}
                                />
                            </>
                        )}
                    </div>
                )}

                {tab === "body" && (
                    <div className="bruno-section bruno-section-fill">
                        <div className="bruno-section-bar">
                            <BrunoSelect
                                value={request.body.mode}
                                options={BODY_OPTS}
                                onChange={(m) => setBody({ mode: m as BodyMode })}
                                className="bruno-mode-dd"
                                title="Body type"
                                menuWidth={150}
                            />
                        </div>
                        {request.body.mode === "none" && <div className="bruno-muted">This request has no body.</div>}
                        {TEXT_MODES.has(request.body.mode) && (
                            <BrunoCode
                                value={request.body.text}
                                lang={bodyLang(request.body.mode)}
                                vars={scope}
                                placeholder="request body"
                                onChange={(v) => setBody({ text: v })}
                                className="bruno-cm-fill"
                            />
                        )}
                        {(request.body.mode === "form-urlencoded" || request.body.mode === "multipart-form" || request.body.mode === "file") && (
                            <KeyValTable
                                rows={request.body.form}
                                scope={scope}
                                onChange={(form) => setBody({ form })}
                                valuePh={request.body.mode === "multipart-form" ? "value or @file(/path)" : "value"}
                            />
                        )}
                    </div>
                )}

                {tab === "headers" && (
                    <div className="bruno-section">
                        <KeyValTable rows={request.headers} scope={scope} onChange={(headers) => set({ headers })} namePh="Header-Name" />
                    </div>
                )}

                {tab === "auth" && (
                    <div className="bruno-section bruno-auth">
                        <label className="bruno-field">
                            <span>Mode</span>
                            <BrunoSelect
                                value={request.auth.mode}
                                options={AUTH_OPTS}
                                onChange={(m) => setAuth({ mode: m as AuthMode })}
                                className="bruno-mode-dd"
                                menuWidth={132}
                            />
                        </label>
                        {request.auth.mode === "bearer" && (
                            <label className="bruno-field">
                                <span>Token</span>
                                <VarInput
                                    value={request.auth.bearer.token}
                                    scope={scope}
                                    placeholder="{{token}}"
                                    onChange={(v) => setAuth({ bearer: { token: v } })}
                                />
                            </label>
                        )}
                        {request.auth.mode === "basic" && (
                            <>
                                <label className="bruno-field">
                                    <span>Username</span>
                                    <input
                                        className="bruno-input"
                                        value={request.auth.basic.username}
                                        spellCheck={false}
                                        onChange={(e) => setAuth({ basic: { ...request.auth.basic, username: e.target.value } })}
                                    />
                                </label>
                                <label className="bruno-field">
                                    <span>Password</span>
                                    <input
                                        className="bruno-input"
                                        type="password"
                                        value={request.auth.basic.password}
                                        onChange={(e) => setAuth({ basic: { ...request.auth.basic, password: e.target.value } })}
                                    />
                                </label>
                            </>
                        )}
                        {request.auth.mode === "apikey" && (
                            <>
                                <label className="bruno-field">
                                    <span>Key</span>
                                    <input
                                        className="bruno-input"
                                        value={request.auth.apikey.key}
                                        spellCheck={false}
                                        onChange={(e) => setAuth({ apikey: { ...request.auth.apikey, key: e.target.value } })}
                                    />
                                </label>
                                <label className="bruno-field">
                                    <span>Value</span>
                                    <VarInput
                                        value={request.auth.apikey.value}
                                        scope={scope}
                                        onChange={(v) => setAuth({ apikey: { ...request.auth.apikey, value: v } })}
                                    />
                                </label>
                                <label className="bruno-field">
                                    <span>Add to</span>
                                    <BrunoSelect
                                        value={request.auth.apikey.placement}
                                        options={APIKEY_PLACEMENT_OPTS}
                                        onChange={(p) => setAuth({ apikey: { ...request.auth.apikey, placement: p as "header" | "queryparams" } })}
                                        className="bruno-mode-dd"
                                        menuWidth={140}
                                    />
                                </label>
                            </>
                        )}
                        {request.auth.mode === "inherit" && <div className="bruno-muted">Inherits auth from the folder / collection.</div>}
                        {request.auth.mode === "none" && <div className="bruno-muted">No authentication.</div>}
                    </div>
                )}

                {tab === "vars" && (
                    <div className="bruno-section">
                        <div className="panel-label">Pre-request vars</div>
                        <KeyValTable
                            rows={request.vars.req}
                            scope={scope}
                            onChange={(req) => set({ vars: { ...request.vars, req } })}
                            valuePh="value or {{expr}}"
                        />
                        <div className="panel-label">Post-response vars</div>
                        <KeyValTable
                            rows={request.vars.res}
                            scope={scope}
                            onChange={(res) => set({ vars: { ...request.vars, res } })}
                            valuePh="res.body.x"
                        />
                    </div>
                )}

                {tab === "script" && (
                    <div className="bruno-section bruno-section-fill">
                        <div className="panel-label">Pre-request</div>
                        <BrunoCode
                            value={request.scripts.pre}
                            lang="javascript"
                            placeholder="// runs before the request"
                            onChange={(pre) => set({ scripts: { ...request.scripts, pre } })}
                        />
                        <div className="panel-label">Post-response</div>
                        <BrunoCode
                            value={request.scripts.post}
                            lang="javascript"
                            placeholder="// runs after the response"
                            onChange={(post) => set({ scripts: { ...request.scripts, post } })}
                        />
                    </div>
                )}

                {tab === "docs" && (
                    <div className="bruno-section bruno-section-fill">
                        <BrunoCode
                            value={request.docs}
                            lang="markdown"
                            placeholder="Markdown documentation for this request"
                            onChange={(docs) => set({ docs })}
                            className="bruno-cm-fill"
                        />
                    </div>
                )}
            </div>
        </div>
    );
}
