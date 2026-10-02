// Reads what recorder.js kept. The recorder patches the page's own fetch and
// console, so its records live among the page's scripts and are read there.
({
    network(limit, filter, method, status, id) {
        const recorder = window.__sikemuxNet;
        if (!recorder) return { recording: false, note: "this tab has not recorded anything; reload the page and retry the action" };
        const all = recorder.entries();
        if (id != null) {
            const call = all.find((entry) => entry.id === Number(id));
            if (!call) throw new Error(`no call ${id} is recorded; it may have been dropped for newer ones`);
            return { recording: true, url: location.href, call };
        }
        const LIST_BODY = 300;
        const BUDGET = 24000;
        const pathOf = (url) => {
            try {
                const parsed = new URL(url, location.href);
                return `${parsed.origin}${parsed.pathname}`.toLowerCase();
            } catch {
                return String(url).toLowerCase();
            }
        };
        const needle = filter ? String(filter).toLowerCase() : "";
        const byPath = needle ? all.filter((entry) => pathOf(entry.url).includes(needle)) : all;
        const byUrl = needle && !byPath.length ? all.filter((entry) => entry.url.toLowerCase().includes(needle)) : byPath;
        const wantedMethod = method ? String(method).toUpperCase() : "";
        const statusMatches = (entry) => {
            if (!status) return true;
            if (status === "failed") return entry.error != null || (entry.status != null && entry.status >= 400);
            if (status === "pending") return entry.durationMs == null;
            if (/^\dxx$/i.test(status)) return entry.status != null && Math.floor(entry.status / 100) === Number(status[0]);
            return entry.status === Number(status);
        };
        const matched = byUrl.filter((entry) => (!wantedMethod || entry.method === wantedMethod) && statusMatches(entry));
        const keep = Math.max(1, Math.min(100, Number(limit) || 20));
        const clipBody = (text) => (text != null && text.length > LIST_BODY ? `${text.slice(0, LIST_BODY)}…` : text);
        const calls = matched.slice(-keep).map((entry) => ({
            ...entry,
            body: clipBody(entry.body),
            requestBody: clipBody(entry.requestBody),
            ...(entry.body != null && entry.body.length > LIST_BODY ? { bodyLength: entry.body.length } : {}),
        }));
        let size = JSON.stringify(calls).length;
        let omitted = 0;
        while (calls.length > 1 && size > BUDGET) {
            size -= JSON.stringify(calls.shift()).length;
            omitted++;
        }
        const result = { recording: true, url: location.href, recorded: all.length, matched: matched.length, calls };
        if (omitted) result.omitted = `${omitted} older calls left out to keep this short; narrow with filter, method or status`;
        if (calls.some((call) => call.bodyLength)) result.note = "bodies are cut to 300 characters; pass a call's id to read it whole";
        return result;
    },
    pending() {
        const recorder = window.__sikemuxNet;
        if (!recorder) return null;
        const now = performance.now();
        // A call open for longer than this is a stream or a long poll, which
        // would otherwise keep the page busy for ever.
        const LONG_POLL = 10000;
        return recorder.entries().filter((entry) => entry.durationMs == null && now - entry.startedAt < LONG_POLL).length;
    },
    console(limit, errorsOnly) {
        const recorder = window.__sikemuxConsole;
        if (!recorder) return { recording: false, note: "this tab has not recorded anything; reload the page and retry the action" };
        const all = recorder.entries();
        const matched = errorsOnly ? all.filter((entry) => entry.level !== "log" && entry.level !== "info" && entry.level !== "debug") : all;
        const keep = Math.max(1, Math.min(200, Number(limit) || 50));
        return { recording: true, url: location.href, recorded: all.length, matched: matched.length, messages: matched.slice(-keep) };
    },
})
