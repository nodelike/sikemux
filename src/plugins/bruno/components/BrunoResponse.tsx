import { IconCheck, IconClose } from "../../../plugin-api/ui";
import type { RunResult } from "../lib/run";
import type { BrunoResTab } from "../state";
import { BrunoCode, type BrunoLang } from "./BrunoCode";

interface Props {
    result: RunResult | null;
    running: boolean;
    tab: BrunoResTab;
    onTab: (t: BrunoResTab) => void;
}

const RES_TABS: BrunoResTab[] = ["body", "headers", "timeline", "tests"];

function statusClass(status: number): string {
    if (status >= 200 && status < 300) return "ok";
    if (status >= 300 && status < 400) return "redir";
    if (status >= 400 && status < 500) return "clienterr";
    return "servererr";
}

function humanSize(n: number): string {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    return `${(n / 1024 / 1024).toFixed(2)} MB`;
}

/** Pretty-print + pick a highlighting language for the response body. */
function formatBody(body: string): { text: string; lang: BrunoLang } {
    const t = body.trim();
    if (t.startsWith("{") || t.startsWith("[")) {
        try {
            return { text: JSON.stringify(JSON.parse(body), null, 2), lang: "json" };
        } catch {
            /* fall through */
        }
    }
    if (t.startsWith("<")) return { text: body, lang: "xml" };
    return { text: body, lang: "text" };
}

export function BrunoResponseView({ result, running, tab, onTab }: Props) {
    const resp = result?.response ?? null;
    const body = resp && !resp.is_binary ? formatBody(resp.body) : null;

    return (
        <div className="bruno-response">
            <div className="bruno-resp-bar">
                {running ? (
                    <span className="bruno-resp-status running">
                        <span className="bruno-row-spin" /> sending…
                    </span>
                ) : resp ? (
                    <>
                        <span className={`bruno-resp-status ${statusClass(resp.status)}`}>
                            {resp.status} {resp.status_text}
                        </span>
                        <span className="bruno-resp-meta">{resp.duration_ms} ms</span>
                        <span className="bruno-resp-meta">{humanSize(resp.size_bytes)}</span>
                    </>
                ) : result?.error ? (
                    <span className="bruno-resp-status servererr">request failed</span>
                ) : (
                    <span className="bruno-resp-status idle">no response yet — hit Send</span>
                )}
                {resp && (
                    <div className="bruno-tabs bruno-resp-tabs">
                        {RES_TABS.map((t) => (
                            <button key={t} className={`bruno-tab${tab === t ? " active" : ""}`} onClick={() => onTab(t)}>
                                {t}
                                {t === "tests" && result && result.tests.length > 0 && (
                                    <span className={`bruno-tab-count${result.tests.some((x) => !x.passed) ? " fail" : " pass"}`}>
                                        {result.tests.length}
                                    </span>
                                )}
                            </button>
                        ))}
                    </div>
                )}
            </div>

            <div className="bruno-resp-body">
                {result?.error && !resp && <div className="bruno-resp-err">{result.error}</div>}
                {resp &&
                    tab === "body" &&
                    (resp.is_binary ? (
                        <div className="bruno-muted">[binary response · {humanSize(resp.size_bytes)}]</div>
                    ) : (
                        <BrunoCode value={body!.text} lang={body!.lang} readOnly className="bruno-cm-fill" />
                    ))}
                {resp && tab === "headers" && (
                    <div className="bruno-kv readonly">
                        {resp.headers.map(([k, v], i) => (
                            <div className="bruno-kv-row" key={i}>
                                <span className="bruno-h-name">{k}</span>
                                <span className="bruno-h-val">{v}</span>
                            </div>
                        ))}
                    </div>
                )}
                {resp && tab === "timeline" && result && (
                    <div className="bruno-timeline">
                        <pre className="bruno-pre">
                            {`${result.request.method} ${result.request.url}\n`}
                            {result.request.headers.map(([k, v]) => `${k}: ${v}`).join("\n")}
                            {`\n\n→ ${resp.status} ${resp.status_text} · ${resp.duration_ms} ms · ${humanSize(resp.size_bytes)}`}
                        </pre>
                        {result.logs.length > 0 && (
                            <div className="bruno-logs">
                                <div className="panel-label">Console</div>
                                {result.logs.map((l, i) => (
                                    <div key={i} className={`bruno-log lvl-${l.level}`}>
                                        <span className="bruno-log-lvl">{l.level}</span>
                                        <span className="bruno-log-text">{l.text}</span>
                                    </div>
                                ))}
                            </div>
                        )}
                    </div>
                )}
                {resp &&
                    tab === "tests" &&
                    (result && result.tests.length > 0 ? (
                        <div className="bruno-tests">
                            {result.tests.map((t, i) => (
                                <div key={i} className={`bruno-test ${t.passed ? "pass" : "fail"}`}>
                                    <span className="bruno-test-dot">{t.passed ? <IconCheck size={12} /> : <IconClose size={12} />}</span>
                                    <span className="bruno-test-name">{t.name}</span>
                                    {t.error && <span className="bruno-test-err">{t.error}</span>}
                                </div>
                            ))}
                        </div>
                    ) : (
                        <div className="bruno-muted">No assertions or tests for this request.</div>
                    ))}
            </div>
        </div>
    );
}
