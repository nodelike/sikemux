// Reads what recorder.js kept. The recorder patches the page's own fetch and
// console, so its records live among the page's scripts and are read there.
({
    network(limit, filter) {
        const recorder = window.__sikemuxNet;
        if (!recorder) return { recording: false, note: "this tab has not recorded anything; reload the page and retry the action" };
        const all = recorder.entries();
        const needle = filter ? String(filter).toLowerCase() : "";
        const matched = needle ? all.filter((entry) => entry.url.toLowerCase().includes(needle)) : all;
        const keep = Math.max(1, Math.min(100, Number(limit) || 20));
        return { recording: true, url: location.href, recorded: all.length, matched: matched.length, calls: matched.slice(-keep) };
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
