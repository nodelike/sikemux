import { type ReactNode } from "react";

const TOK = new RegExp(
    [
        "(?<ts>\\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}:\\d{2}(?:\\.\\d+)?(?:Z|[+-]\\d{2}:?\\d{2})?)",
        "(?<clock>\\b\\d{2}:\\d{2}:\\d{2}(?:\\.\\d+)?\\b)",
        "(?<lvl_err>\\[?\\b(?:ERROR|ERR|FATAL|CRITICAL|PANIC|SEVERE)\\b\\]?)",
        "(?<lvl_warn>\\[?\\b(?:WARN|WARNING)\\b\\]?)",
        "(?<lvl_info>\\[?\\b(?:INFO|INFORMATIONAL|NOTICE)\\b\\]?)",
        "(?<lvl_debug>\\[?\\b(?:DEBUG|TRACE|VERBOSE)\\b\\]?)",
        "(?<method>\\b(?:GET|POST|PUT|DELETE|PATCH|HEAD|OPTIONS)\\b)",
        "(?<status5>\\b5\\d{2}\\b)",
        "(?<status4>\\b4\\d{2}\\b)",
        "(?<status3>\\b3\\d{2}\\b)",
        "(?<status2>\\b2\\d{2}\\b)",
        "(?<url>(?:https?|wss?|s3):\\/\\/[\\w@:%._+~#=/?&-]+)",
        "(?<uuid>\\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\\b)",
        "(?<ip>\\b\\d{1,3}\\.\\d{1,3}\\.\\d{1,3}\\.\\d{1,3}(?::\\d+)?\\b)",
        '(?<jkey>"[^"\\\\]+"\\s*:)',
        '(?<jstr>"(?:[^"\\\\]|\\\\.)*")',
        "(?<bool>\\b(?:true|false|null|None|True|False)\\b)",
        "(?<num>\\b\\d+(?:\\.\\d+)?\\b)",
        "(?<tag>\\[[^\\]\\s]{1,40}\\])",
    ].join("|"),
    "gi",
);

const CLASS_FOR_GROUP: Record<string, string> = {
    ts: "lt-ts",
    clock: "lt-ts",
    lvl_err: "lt-lvl-err",
    lvl_warn: "lt-lvl-warn",
    lvl_info: "lt-lvl-info",
    lvl_debug: "lt-lvl-debug",
    method: "lt-method",
    status5: "lt-status-5",
    status4: "lt-status-4",
    status3: "lt-status-3",
    status2: "lt-status-2",
    url: "lt-url",
    uuid: "lt-uuid",
    ip: "lt-ip",
    jkey: "lt-jkey",
    jstr: "lt-jstr",
    bool: "lt-bool",
    num: "lt-num",
    tag: "lt-tag",
};

export function highlightLog(text: string): ReactNode[] {
    if (!text) return [text];
    const out: ReactNode[] = [];
    let cursor = 0;
    let key = 0;
    for (const m of text.matchAll(TOK)) {
        const idx = m.index ?? 0;
        if (idx > cursor) out.push(text.slice(cursor, idx));
        const groups = m.groups ?? {};
        let cls: string | undefined;
        let value: string | undefined;
        for (const g of Object.keys(CLASS_FOR_GROUP)) {
            if (groups[g] !== undefined) {
                cls = CLASS_FOR_GROUP[g];
                value = groups[g];
                break;
            }
        }
        if (cls && value !== undefined) {
            out.push(
                <span key={key++} className={`lt ${cls}`}>
                    {value}
                </span>,
            );
        } else {
            out.push(m[0]);
        }
        cursor = idx + m[0].length;
    }
    if (cursor < text.length) out.push(text.slice(cursor));
    return out;
}
