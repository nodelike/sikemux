//! Host-side sandboxed JS orchestration (`browser_drive`).
//!
//! Runs an agent-supplied async script in an isolated QuickJS runtime with strict
//! timeout and memory limits. Every browser action routes strictly through Sikemux's
//! gated `tools::run` primitives so `CheckWebWrite`, `person::leave_be`, and trusted
//! OS input are enforced with zero duplicated security logic.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::{Duration, Instant};

use rquickjs::function::Async;
use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, CaughtError, Function, Promise};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use tokio::sync::{mpsc, oneshot};

use super::*;

pub(super) const DEFAULT_DRIVE_TIMEOUT_MS: u64 = 45_000;
const DRIVE_MEMORY_LIMIT: usize = 32 * 1024 * 1024;
const DRIVE_STACK_LIMIT: usize = 512 * 1024;
const MAX_DRIVE_STEPS: usize = 200;

/// JS SDK exposed as the `browser` global inside `browser_drive`.
/// Pure JS only — no filesystem, network, env, or DOM globals exist in this runtime.
const DRIVE_SDK_JS: &str = r##"
(() => {
    const encodeForm = (s) =>
        encodeURIComponent(String(s))
            .replace(/%20/g, "+")
            .replace(/[!'()~]/g, (c) => "%" + c.charCodeAt(0).toString(16).toUpperCase());
    const decodeForm = (s) => {
        try { return decodeURIComponent(String(s).replace(/\+/g, " ")); }
        catch { return String(s).replace(/\+/g, " "); }
    };

    class URLSearchParams {
        constructor(init, onChange) {
            this._pairs = [];
            this._onChange = typeof onChange === "function" ? onChange : null;
            if (init == null) return;
            if (init instanceof URLSearchParams) {
                for (const [k, v] of init._pairs) this._pairs.push([k, v]);
            } else if (typeof init === "string") {
                const raw = init.startsWith("?") ? init.slice(1) : init;
                if (raw) {
                    for (const part of raw.split("&")) {
                        if (!part) continue;
                        const eq = part.indexOf("=");
                        if (eq < 0) this._pairs.push([decodeForm(part), ""]);
                        else this._pairs.push([decodeForm(part.slice(0, eq)), decodeForm(part.slice(eq + 1))]);
                    }
                }
            } else if (Array.isArray(init) || (typeof init === "object" && typeof init[Symbol.iterator] === "function")) {
                for (const entry of init) {
                    if (!entry || entry.length !== 2) throw new TypeError("URLSearchParams sequence entries must have 2 elements");
                    this._pairs.push([String(entry[0]), String(entry[1])]);
                }
            } else if (typeof init === "object") {
                for (const [k, v] of Object.entries(init)) {
                    this._pairs.push([String(k), String(v)]);
                }
            }
        }
        _notify() {
            if (this._onChange) this._onChange(this.toString());
        }
        get size() { return this._pairs.length; }
        append(name, value) {
            this._pairs.push([String(name), String(value)]);
            this._notify();
        }
        delete(name, value) {
            const key = String(name);
            const hasVal = value !== undefined;
            const val = hasVal ? String(value) : "";
            this._pairs = this._pairs.filter(([k, v]) => k !== key || (hasVal && v !== val));
            this._notify();
        }
        get(name) {
            const key = String(name);
            const pair = this._pairs.find(([k]) => k === key);
            return pair ? pair[1] : null;
        }
        getAll(name) {
            const key = String(name);
            return this._pairs.filter(([k]) => k === key).map(([, v]) => v);
        }
        has(name, value) {
            const key = String(name);
            const hasVal = value !== undefined;
            const val = hasVal ? String(value) : "";
            return this._pairs.some(([k, v]) => k === key && (!hasVal || v === val));
        }
        set(name, value) {
            const key = String(name);
            const val = String(value);
            let replaced = false;
            const next = [];
            for (const pair of this._pairs) {
                if (pair[0] === key) {
                    if (!replaced) {
                        next.push([key, val]);
                        replaced = true;
                    }
                } else {
                    next.push(pair);
                }
            }
            if (!replaced) next.push([key, val]);
            this._pairs = next;
            this._notify();
        }
        sort() {
            this._pairs.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
            this._notify();
        }
        *entries() { for (const [k, v] of this._pairs) yield [k, v]; }
        *keys() { for (const [k] of this._pairs) yield k; }
        *values() { for (const [, v] of this._pairs) yield v; }
        forEach(cb, thisArg) {
            for (const [k, v] of this._pairs) cb.call(thisArg, v, k, this);
        }
        [Symbol.iterator]() { return this.entries(); }
        toString() {
            return this._pairs.map(([k, v]) => `${encodeForm(k)}=${encodeForm(v)}`).join("&");
        }
    }

    const parseUrlParts = (input, base) => {
        const raw = __sikemuxParseUrl(String(input), base === undefined ? null : String(base));
        const env = JSON.parse(raw);
        if (env.error !== undefined) throw new TypeError(env.error);
        return env.ok;
    };

    const rebuildUrl = (parts) => {
        let auth = "";
        if (parts.username || parts.password) {
            auth = parts.username + (parts.password ? `:${parts.password}` : "") + "@";
        }
        const authority = parts.host || auth ? `//${auth}${parts.host}` : "";
        return `${parts.protocol}${authority}${parts.pathname}${parts.search}${parts.hash}`;
    };

    class URL {
        constructor(input, base) {
            this._parts = parseUrlParts(input, base);
            this._params = new URLSearchParams(this._parts.search, (qs) => {
                this._parts.search = qs ? `?${qs}` : "";
                this._parts = parseUrlParts(rebuildUrl(this._parts));
            });
        }
        static canParse(input, base) {
            try { parseUrlParts(input, base); return true; } catch { return false; }
        }
        static parse(input, base) {
            try { return new URL(input, base); } catch { return null; }
        }
        _syncParams() {
            const next = new URLSearchParams(this._parts.search);
            this._params._pairs = next._pairs;
        }
        _update(patch) {
            const candidate = { ...this._parts, ...patch };
            try {
                this._parts = parseUrlParts(rebuildUrl(candidate));
                this._syncParams();
            } catch {}
        }
        get href() { return this._parts.href; }
        set href(v) {
            this._parts = parseUrlParts(v);
            this._syncParams();
        }
        get origin() { return this._parts.origin; }
        get protocol() { return this._parts.protocol; }
        set protocol(v) {
            const s = String(v).trim();
            if (!s) return;
            this._update({ protocol: s.endsWith(":") ? s : `${s}:` });
        }
        get username() { return this._parts.username; }
        set username(v) { this._update({ username: encodeURIComponent(String(v)) }); }
        get password() { return this._parts.password; }
        set password(v) { this._update({ password: encodeURIComponent(String(v)) }); }
        get host() { return this._parts.host; }
        set host(v) { this._update({ host: String(v) }); }
        get hostname() { return this._parts.hostname; }
        set hostname(v) {
            const h = String(v);
            this._update({ host: this._parts.port ? `${h}:${this._parts.port}` : h });
        }
        get port() { return this._parts.port; }
        set port(v) {
            const p = String(v).trim();
            this._update({ host: p ? `${this._parts.hostname}:${p}` : this._parts.hostname });
        }
        get pathname() { return this._parts.pathname; }
        set pathname(v) {
            const p = String(v);
            this._update({ pathname: p.startsWith("/") ? p : `/${p}` });
        }
        get search() { return this._parts.search; }
        set search(v) {
            const s = String(v);
            this._update({ search: !s || s === "?" ? "" : (s.startsWith("?") ? s : `?${s}`) });
        }
        get searchParams() { return this._params; }
        get hash() { return this._parts.hash; }
        set hash(v) {
            const s = String(v);
            this._update({ hash: !s || s === "#" ? "" : (s.startsWith("#") ? s : `#${s}`) });
        }
        toString() { return this._parts.href; }
        toJSON() { return this._parts.href; }
    }

    globalThis.URL = URL;
    globalThis.URLSearchParams = URLSearchParams;

    let __nextTimerId = 1;
    const __activeTimers = new Set();
    const MAX_TIMERS = 64;
    globalThis.setTimeout = (fn, ms, ...args) => {
        if (typeof fn !== "function") {
            throw new TypeError("setTimeout callback must be a function");
        }
        if (__activeTimers.size >= MAX_TIMERS) {
            throw new RangeError(`at most ${MAX_TIMERS} active timers allowed in browser_drive`);
        }
        const id = __nextTimerId++;
        const delay = Math.max(0, Number(ms) || 0);
        __activeTimers.add(id);
        __sikemuxWaitTimer(id, delay).then((fire) => {
            if (!__activeTimers.delete(id) || !fire) return;
            fn(...args);
        });
        return id;
    };
    globalThis.clearTimeout = (id) => {
        if (__activeTimers.delete(id)) {
            __sikemuxCancelTimer(id);
        }
    };

    const normalizeTarget = (arg, opts) => {
        if (typeof arg === "number") return { index: arg, ...(opts || {}) };
        if (typeof arg === "string") return { text: arg, ...(opts || {}) };
        return { ...(arg || {}), ...(opts || {}) };
    };
    const isGateError = (msg) => typeof msg === "string" && msg.startsWith("Waits for the person");

    const invoke = async (method, params) => {
        const optional = Boolean(params && params.optional === true);
        const clean = params ? { ...params } : {};
        delete clean.optional;
        try {
            const raw = await __sikemuxInvoke(method, JSON.stringify(clean));
            const env = JSON.parse(raw);
            if (env && env.error !== undefined) throw new Error(env.error);
            return env.ok;
        } catch (err) {
            const msg = String((err && err.message) || err);
            if (optional && !isGateError(msg)) {
                return { skipped: true, error: msg };
            }
            throw err;
        }
    };

    const parseTypeParams = (name, first, second, third) => {
        const sig = `tab.${name}(target, text, opts?) or tab.${name}({ field|selector|index?, text, replace?, submit? })`;
        if (first === undefined) {
            throw new Error(`tab.${name} requires text: ${sig}`);
        }
        let p;
        const isScalarText = (v) => typeof v === "string" || typeof v === "number";
        if (typeof first === "object" && first !== null) {
            if (isScalarText(second)) {
                p = { ...first, text: String(second), ...(third || {}) };
            } else if (typeof second === "object" && second !== null) {
                p = { ...first, ...second };
            } else {
                p = { ...first };
            }
        } else if (typeof first === "number") {
            if (isScalarText(second)) {
                p = { index: first, text: String(second), ...(third || {}) };
            } else if (typeof second === "object" && second !== null) {
                p = { index: first, ...second };
            } else {
                throw new Error(`tab.${name} requires text: ${sig}`);
            }
        } else if (typeof first === "string") {
            if (isScalarText(second)) {
                p = { field: first, text: String(second), ...(third || {}) };
            } else if (typeof second === "object" && second !== null && second.text !== undefined) {
                p = { field: first, ...second };
            } else {
                p = { text: first, ...(typeof second === "object" && second !== null ? second : {}) };
            }
        } else {
            throw new Error(`tab.${name} requires text: ${sig}`);
        }
        if (typeof p.text === "number") p.text = String(p.text);
        if (typeof p.text !== "string") {
            const valueHint = p.value !== undefined ? ' (use "text", not "value")' : "";
            throw new Error(`tab.${name} requires text${valueHint}: ${sig}`);
        }
        return p;
    };

    const TAB_METHODS = "goto, click, hover, type, fill, select, upload, dialog, download, press, scroll, wait, state, find, eval, screenshot, console, network, viewport, close";
    const TAB_HINTS = {
        navigate: "use tab.goto(url, opts?)",
        go: "use tab.goto(url, opts?) or tab.goto({ go: 'back'|'forward'|'reload' })",
        back: "use tab.goto({ go: 'back' })",
        forward: "use tab.goto({ go: 'forward' })",
        reload: "use tab.goto({ go: 'reload' })",
        evaluate: "use tab.eval(script, opts?)",
        waitFor: "use tab.wait(arg)",
        querySelector: 'use tab.eval("document.querySelector(...)") or tab.find(query)',
        setValue: "use tab.fill(target, text, opts?)",
    };

    const makeTab = (initialId, initialMeta) => {
        let currentId = initialId || null;
        let currentUrl = (initialMeta && initialMeta.url) || "";
        let currentTitle = (initialMeta && initialMeta.title) || "";
        let currentProfile = (initialMeta && initialMeta.profile) || null;

        const callOnTab = async (method, params) => {
            const payload = { ...(params || {}) };
            if (currentId && payload.tabId === undefined) payload.tabId = currentId;
            const res = await invoke(method, payload);
            if (res && typeof res === "object") {
                if (typeof res.tabId === "string" && res.tabId) currentId = res.tabId;
                if (typeof res.url === "string") currentUrl = res.url;
                if (typeof res.title === "string") currentTitle = res.title;
                if (typeof res.profile === "string" && res.profile) currentProfile = res.profile;
            }
            return res;
        };

        const api = {
            get id() { return currentId; },
            get url() { return currentUrl; },
            get title() { return currentTitle; },
            get profile() { return currentProfile; },
            goto(urlOrOpts, opts) {
                const p = typeof urlOrOpts === "string" ? { url: urlOrOpts, ...(opts || {}) } : { ...(urlOrOpts || {}) };
                if (typeof p.url !== "string" && typeof p.go !== "string") {
                    throw new Error('tab.goto requires url or go: tab.goto(url, opts?) or tab.goto({ url|go, waitFor? })');
                }
                if (p.report === undefined) p.report = "outcome";
                return callOnTab("browser.navigate", p);
            },
            click(target, opts) {
                const p = normalizeTarget(target, opts);
                if (p.index === undefined && p.text === undefined && p.selector === undefined && (p.x === undefined || p.y === undefined)) {
                    throw new Error('tab.click requires a target: tab.click(target, opts?) where target is index, text, or { selector|text|index|x,y }');
                }
                if (p.report === undefined) p.report = "outcome";
                return callOnTab("browser.click", p);
            },
            hover(target, opts) {
                const p = normalizeTarget(target, opts);
                if (p.index === undefined && p.text === undefined && p.selector === undefined && (p.x === undefined || p.y === undefined)) {
                    throw new Error('tab.hover requires a target: tab.hover(target, opts?) where target is index, text, or { selector|text|index|x,y }');
                }
                p.hover = true;
                if (p.report === undefined) p.report = "outcome";
                return callOnTab("browser.click", p);
            },
            type(first, second, third) {
                const p = parseTypeParams("type", first, second, third);
                if (p.report === undefined) p.report = "outcome";
                return callOnTab("browser.type", p);
            },
            fill(first, second, third) {
                const p = parseTypeParams("fill", first, second, third);
                if (p.replace === undefined) p.replace = true;
                if (p.report === undefined) p.report = "outcome";
                return callOnTab("browser.type", p);
            },
            select(target, value, opts) {
                let p;
                if (typeof target === "object" && target !== null && value === undefined) {
                    p = { ...target };
                } else if (typeof target === "number") {
                    p = { index: target, value, ...(opts || {}) };
                } else if (typeof target === "string") {
                    p = { field: target, value, ...(opts || {}) };
                } else {
                    p = { ...(target || {}), value, ...(opts || {}) };
                }
                if (p.value === undefined) {
                    throw new Error('tab.select requires target and value: tab.select(target, value, opts?) or tab.select({ field|selector|index, value })');
                }
                if (p.report === undefined) p.report = "outcome";
                return callOnTab("browser.select", p);
            },
            upload(target, files, opts) {
                const toPaths = (val) => Array.isArray(val) ? val.map(String) : [String(val)];
                let p;
                if (files === undefined) {
                    if (Array.isArray(target) || typeof target === "string") {
                        p = { paths: toPaths(target) };
                    } else {
                        p = { ...(target || {}) };
                        if (p.files !== undefined && p.paths === undefined) {
                            p.paths = toPaths(p.files);
                            delete p.files;
                        }
                    }
                } else if (target == null) {
                    p = { paths: toPaths(files), ...(opts || {}) };
                } else if (typeof target === "number") {
                    p = { index: target, paths: toPaths(files), ...(opts || {}) };
                } else if (typeof target === "string") {
                    p = { selector: target, paths: toPaths(files), ...(opts || {}) };
                } else {
                    p = { ...target, paths: toPaths(files), ...(opts || {}) };
                }
                if (!Array.isArray(p.paths) || p.paths.length === 0) {
                    throw new Error('tab.upload requires files: tab.upload(target, files, opts?) or tab.upload(files)');
                }
                if (p.report === undefined) p.report = "outcome";
                return callOnTab("browser.upload", p);
            },
            dialog(action, text, opts) {
                let p;
                if (typeof action === "object" && action !== null) {
                    p = { ...action };
                } else {
                    const accept =
                        action === true ||
                        action === "accept" ||
                        action === "ok";
                    const dismiss =
                        action === false ||
                        action === "dismiss" ||
                        action === "cancel";
                    if (!accept && !dismiss) {
                        throw new Error('tab.dialog requires action: tab.dialog("accept"|"dismiss", text?)');
                    }
                    p = { accept };
                    if (typeof text === "string") p.text = text;
                    else if (typeof text === "object" && text !== null) Object.assign(p, text);
                    if (opts && typeof opts === "object") Object.assign(p, opts);
                }
                if (p.report === undefined) p.report = "outcome";
                return callOnTab("browser.dialog", p);
            },
            download(url, opts) {
                const p = typeof url === "string" ? { url, ...(opts || {}) } : { ...(url || {}) };
                if (typeof p.url !== "string" || !p.url) {
                    throw new Error('tab.download requires url: tab.download(url, opts?) or tab.download({ url, filename? })');
                }
                return callOnTab("browser.download", p);
            },
            press(key, opts) {
                const p = typeof key === "object" && key !== null ? { ...key, ...(opts || {}) } : { key, ...(opts || {}) };
                if (typeof p.key !== "string" || !p.key) {
                    throw new Error('tab.press requires key: tab.press(key, opts?) or tab.press({ key })');
                }
                if (p.report === undefined) p.report = "outcome";
                return callOnTab("browser.press", p);
            },
            scroll(arg) {
                let p;
                if (typeof arg === "number") p = { deltaY: arg };
                else if (arg === "top" || arg === "bottom") p = { to: arg };
                else p = { ...(arg || {}) };
                return callOnTab("browser.scroll", p);
            },
            wait(arg) {
                let p;
                if (typeof arg === "number") p = { ms: arg };
                else if (typeof arg === "string") p = { text: arg };
                else p = { ...(arg || {}) };
                if (p.report === undefined) p.report = "outcome";
                return callOnTab("browser.wait", p);
            },
            state(arg) {
                const p = typeof arg === "string" ? { selector: arg } : { ...(arg || {}) };
                return callOnTab("browser.state", p);
            },
            find(query, role) {
                const p = typeof query === "object" && query !== null ? { ...query } : { query, ...(role ? { role } : {}) };
                if (typeof p.query !== "string" || !p.query) {
                    throw new Error('tab.find requires query: tab.find(query, role?) or tab.find({ query, role? })');
                }
                return callOnTab("browser.find", p);
            },
            eval(script, opts) {
                let p;
                if (typeof script === "string") {
                    p = { script, ...(opts || {}) };
                } else if (typeof script === "object" && script !== null) {
                    p = { ...script, ...(opts || {}) };
                } else {
                    throw new Error('tab.eval requires a JS string: tab.eval(script, opts?) or tab.eval({ script, timeoutMs? })');
                }
                if (typeof p.script !== "string" || !p.script.trim()) {
                    throw new Error('tab.eval requires a JS string: tab.eval(script, opts?) or tab.eval({ script, timeoutMs? })');
                }
                return callOnTab("browser.evaluate", p);
            },
            screenshot(opts) {
                return callOnTab("browser.screenshot", opts || {});
            },
            console(opts) {
                return callOnTab("browser.console", opts || {});
            },
            network(opts) {
                return callOnTab("browser.network", opts || {});
            },
            viewport(arg) {
                const p = typeof arg === "string" ? { preset: arg } : { ...(arg || {}) };
                return callOnTab("browser.viewport", p);
            },
            close() {
                return callOnTab("browser.tab.close", {});
            },
        };

        return new Proxy(api, {
            get(target, prop, receiver) {
                if (typeof prop === "symbol" || prop in target || prop === "then" || prop === "toJSON") {
                    return Reflect.get(target, prop, receiver);
                }
                const hint = TAB_HINTS[prop] ? ` (${TAB_HINTS[prop]})` : "";
                return () => {
                    throw new TypeError(`tab.${prop} is not a method${hint}. Valid tab methods: ${TAB_METHODS}`);
                };
            },
        });
    };

    const browserApi = Object.freeze({
        tabs() {
            return JSON.parse(__sikemuxTabs());
        },
        profiles() {
            return JSON.parse(__sikemuxProfiles());
        },
        async open(url, opts) {
            const p = typeof url === "string"
                ? { url, newTab: true, ...(opts || {}) }
                : { newTab: true, ...(url || {}), ...(opts || {}) };
            if (typeof p.url !== "string" || !p.url) {
                throw new Error('browser.open requires url: browser.open(url, opts?) or browser.open({ url, profile?, background?, waitFor? })');
            }
            if (p.report === undefined) p.report = "outcome";
            const res = await invoke("browser.navigate", p);
            return makeTab(res && res.tabId, res);
        },
        tab(id) {
            return makeTab(id || null, null);
        },
    });

    globalThis.browser = new Proxy(browserApi, {
        get(target, prop, receiver) {
            if (typeof prop === "symbol" || prop in target || prop === "then" || prop === "toJSON") {
                return Reflect.get(target, prop, receiver);
            }
            return () => {
                throw new TypeError(`browser.${prop} is not a method. Valid browser methods: tabs(), profiles(), open(url, opts?), tab(id?)`);
            };
        },
    });

    globalThis.sleep = (ms) => __sikemuxSleep(Math.max(0, Number(ms) || 0));
})();
"##;

/// Methods `browser_drive` is allowed to invoke through the gated host dispatcher.
const ALLOWED_DRIVE_METHODS: &[&str] = &[
    "browser.navigate",
    "browser.state",
    "browser.find",
    "browser.click",
    "browser.type",
    "browser.select",
    "browser.press",
    "browser.drag",
    "browser.upload",
    "browser.dialog",
    "browser.download",
    "browser.viewport",
    "browser.scroll",
    "browser.evaluate",
    "browser.network",
    "browser.console",
    "browser.screenshot",
    "browser.annotate",
    "browser.wait",
    "browser.tab.close",
];

pub(crate) trait DriveBackend {
    fn call<'a>(
        &'a self,
        method: String,
        params: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + 'a>>;

    fn tabs(&self) -> Value;

    fn profiles(&self) -> Value {
        json!([])
    }
}

struct LiveBackend<'a> {
    manager: &'a BrowserManager,
    agent_id: &'a str,
    project: &'a str,
    check: &'a CheckWrite,
    vault: Option<&'a ResolveVault>,
    resolve_profile: Option<&'a ResolveProfile>,
    profiles: &'a [AgentProfileInfo],
    allowed_sites: Value,
    upload_from: Value,
    default_tab: Option<String>,
}

impl<'a> DriveBackend for LiveBackend<'a> {
    fn call<'b>(
        &'b self,
        method: String,
        mut params: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + 'b>> {
        if method == "browser.evaluate" {
            put(&mut params, "allowedSites", self.allowed_sites.clone());
        }
        if method == "browser.upload" {
            put(&mut params, "uploadFrom", self.upload_from.clone());
        }
        if let Some(default_tab) = self.default_tab.as_deref() {
            let opens_new = method == "browser.navigate"
                && params.get("newTab").and_then(Value::as_bool) == Some(true);
            if !opens_new && params.get("tabId").is_none() {
                put(&mut params, "tabId", json!(default_tab));
            }
        }
        Box::pin(async move {
            let tab_hint = requested_tab(&params).map(str::to_owned);
            let mut marks = Vec::new();
            if method != "browser.tab.close" {
                marks.extend(
                    self.manager
                        .mark_acting_tab(self.agent_id, tab_hint.as_deref()),
                );
            }
            let outcome = match method.as_str() {
                "browser.select" => {
                    select_option(self.manager, self.agent_id, self.check, &params).await
                }
                "browser.download" => download_asset(self.manager, self.agent_id, &params).await,
                _ => {
                    Box::pin(run(
                        self.manager,
                        self.agent_id,
                        self.project,
                        self.check,
                        self.vault,
                        self.resolve_profile,
                        self.profiles,
                        &method,
                        &params,
                    ))
                    .await
                }
            };
            if method != "browser.tab.close" {
                let acted_tab = outcome
                    .as_ref()
                    .ok()
                    .and_then(|value| value.get("tabId").and_then(Value::as_str))
                    .or(tab_hint.as_deref());
                marks.extend(self.manager.mark_acting_tab(self.agent_id, acted_tab));
                self.manager.release_acting(self.agent_id, marks);
            }
            let secrets = self.manager.blind_secrets_for(self.agent_id, None);
            let mut outcome = outcome;
            match &mut outcome {
                Ok(val) => redact_value(val, &secrets),
                Err(err) => *err = redact_text(err, &secrets),
            }
            outcome
        })
    }

    fn tabs(&self) -> Value {
        let snapshot =
            self.manager
                .snapshot(self.agent_id)
                .unwrap_or_else(|_| crate::BrowserSnapshot {
                    tabs: Vec::new(),
                    active_tab_id: None,
                });
        let yours: std::collections::HashSet<String> = self
            .manager
            .agents
            .locked()
            .get(self.agent_id)
            .map(|agent| {
                snapshot
                    .tabs
                    .iter()
                    .filter(|tab| agent.strip.yours(&tab.id))
                    .map(|tab| tab.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        json!(snapshot
            .tabs
            .iter()
            .map(|tab| {
                let mut obj = Map::new();
                obj.insert("id".into(), json!(tab.id));
                obj.insert("title".into(), json!(tab.title));
                obj.insert("url".into(), json!(tab.url));
                obj.insert("active".into(), json!(tab.active));
                obj.insert("loading".into(), json!(tab.loading));
                obj.insert("yours".into(), json!(yours.contains(&tab.id)));
                if let Some(slug) = profile_slug_of(tab.profile_id.as_deref(), self.profiles) {
                    obj.insert("profile".into(), json!(slug));
                }
                Value::Object(obj)
            })
            .collect::<Vec<_>>())
    }

    fn profiles(&self) -> Value {
        if self.profiles.len() <= 1 {
            json!([])
        } else {
            profiles_json(self.profiles)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_tool(
    manager: &BrowserManager,
    agent_id: &str,
    project: &str,
    check: &CheckWrite,
    vault: Option<&ResolveVault>,
    resolve_profile: Option<&ResolveProfile>,
    profiles: &[AgentProfileInfo],
    params: &Value,
) -> Result<Value, String> {
    let script = params
        .get("script")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or("script is required")?;
    let timeout_ms = params
        .get("timeoutMs")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_DRIVE_TIMEOUT_MS)
        .clamp(100, MAX_BLOCKING_MS);
    let backend = LiveBackend {
        manager,
        agent_id,
        project,
        check,
        vault,
        resolve_profile,
        profiles,
        allowed_sites: params
            .get("allowedSites")
            .cloned()
            .unwrap_or_else(|| json!([])),
        upload_from: params
            .get("uploadFrom")
            .cloned()
            .unwrap_or_else(|| json!([project])),
        default_tab: requested_tab(params).map(str::to_owned),
    };
    Ok(run_drive(&backend, script, Duration::from_millis(timeout_ms)).await)
}

fn is_held_write_error(error: &str) -> bool {
    error.starts_with("Waits for the person")
}

fn step_summary(method: &str, params: &Value) -> String {
    let str_of = |key: &str| params.get(key).and_then(Value::as_str);
    match method {
        "browser.navigate" => {
            if let Some(url) = str_of("url") {
                format!("open {url}")
            } else if let Some(go) = str_of("go") {
                format!("go {go}")
            } else {
                "navigate".into()
            }
        }
        "browser.click" => {
            let verb = if params.get("hover").and_then(Value::as_bool) == Some(true) {
                "hover"
            } else {
                "click"
            };
            if let Some(text) = str_of("text") {
                format!("{verb} \"{text}\"")
            } else if let Some(sel) = str_of("selector") {
                format!("{verb} {sel}")
            } else if let Some(idx) = params.get("index").and_then(Value::as_u64) {
                format!("{verb} [{idx}]")
            } else {
                verb.into()
            }
        }
        "browser.type" => {
            if let Some(field) = str_of("field") {
                format!("type into \"{field}\"")
            } else if let Some(sel) = str_of("selector") {
                format!("type into {sel}")
            } else {
                "type".into()
            }
        }
        "browser.select" => {
            let target = str_of("field")
                .or_else(|| str_of("selector"))
                .unwrap_or("select");
            format!("select in \"{target}\"")
        }
        "browser.upload" => {
            let count = params
                .get("paths")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            format!("upload {count} file(s)")
        }
        "browser.dialog" => {
            let verb = if params.get("accept").and_then(Value::as_bool) == Some(true) {
                "accept"
            } else {
                "dismiss"
            };
            format!("{verb} dialog")
        }
        "browser.download" => format!("download {}", str_of("url").unwrap_or("asset")),
        "browser.press" => format!("press {}", str_of("key").unwrap_or("key")),
        "browser.wait" => {
            if let Some(text) = str_of("text") {
                format!("wait for \"{text}\"")
            } else if let Some(sel) = str_of("selector") {
                format!("wait for {sel}")
            } else if let Some(url) = str_of("url") {
                format!("wait for url {url}")
            } else {
                "wait".into()
            }
        }
        _ => method.strip_prefix("browser.").unwrap_or(method).to_owned(),
    }
}

struct InvokeRequest {
    method: String,
    params: Value,
    reply: oneshot::Sender<Result<Value, String>>,
}

struct HeldWrite {
    step: usize,
    method: String,
    tab_id: Option<String>,
    summary: String,
    error: String,
}

struct InFlight<'a> {
    step: usize,
    method: String,
    tab_id: Option<String>,
    summary: String,
    reply: Option<oneshot::Sender<Result<Value, String>>>,
    future: Pin<Box<dyn Future<Output = Result<Value, String>> + 'a>>,
}

pub(crate) async fn run_drive(
    backend: &dyn DriveBackend,
    script: &str,
    timeout: Duration,
) -> Value {
    let runtime = match AsyncRuntime::new() {
        Ok(rt) => rt,
        Err(error) => {
            return json!({
                "ok": false,
                "error": format!("could not start browser_drive runtime: {error}"),
                "completedSteps": [],
            });
        }
    };
    runtime.set_memory_limit(DRIVE_MEMORY_LIMIT).await;
    runtime.set_max_stack_size(DRIVE_STACK_LIMIT).await;

    let deadline = Instant::now() + timeout;
    let gate_aborted = Arc::new(AtomicBool::new(false));
    let timed_out = Arc::new(AtomicBool::new(false));
    {
        let gate_flag = Arc::clone(&gate_aborted);
        let timeout_flag = Arc::clone(&timed_out);
        runtime
            .set_interrupt_handler(Some(Box::new(move || {
                if gate_flag.load(Ordering::Acquire) {
                    return true;
                }
                if Instant::now() >= deadline {
                    timeout_flag.store(true, Ordering::Release);
                    return true;
                }
                false
            })))
            .await;
    }

    let context = match AsyncContext::full(&runtime).await {
        Ok(ctx) => ctx,
        Err(error) => {
            return json!({
                "ok": false,
                "error": format!("could not initialize browser_drive context: {error}"),
                "completedSteps": [],
            });
        }
    };

    let (req_tx, mut req_rx) = mpsc::unbounded_channel::<InvokeRequest>();
    let tabs_snapshot = Arc::new(Mutex::new(backend.tabs()));
    let tabs_for_js = Arc::clone(&tabs_snapshot);
    let profiles_for_js =
        serde_json::to_string(&backend.profiles()).unwrap_or_else(|_| "[]".to_owned());
    let timers: Arc<Mutex<std::collections::HashMap<u64, oneshot::Sender<()>>>> =
        Arc::new(Mutex::new(std::collections::HashMap::new()));
    let timers_for_js = Arc::clone(&timers);
    let gate_for_timers = Arc::clone(&gate_aborted);
    let script_owned = script.to_owned();

    let js_fut = async move {
        let tx = req_tx;
        let result: Result<String, String> = context
            .async_with(async |ctx| {
                let globals = ctx.globals();

                let tx_invoke = tx.clone();
                let invoke_fn = Function::new(
                    ctx.clone(),
                    Async(move |method: String, params_json: String| {
                        let tx = tx_invoke.clone();
                        async move {
                            let answer = async {
                                let params: Value = serde_json::from_str(&params_json)
                                    .map_err(|e| format!("invalid parameters: {e}"))?;
                                let (reply_tx, reply_rx) = oneshot::channel();
                                tx.send(InvokeRequest {
                                    method,
                                    params,
                                    reply: reply_tx,
                                })
                                .map_err(|_| "browser_drive host stopped".to_owned())?;
                                reply_rx
                                    .await
                                    .map_err(|_| "browser_drive call aborted".to_owned())?
                            }
                            .await;
                            let envelope = match answer {
                                Ok(value) => json!({ "ok": value }),
                                Err(error) => json!({ "error": error }),
                            };
                            serde_json::to_string(&envelope)
                                .unwrap_or_else(|_| r#"{"error":"could not encode result"}"#.into())
                        }
                    }),
                )
                .map_err(|e| format!("could not register invoke: {e}"))?;
                globals
                    .set("__sikemuxInvoke", invoke_fn)
                    .map_err(|e| format!("could not set invoke: {e}"))?;

                let tabs_handle = Arc::clone(&tabs_for_js);
                let tabs_fn = Function::new(ctx.clone(), move || -> String {
                    let value = tabs_handle.locked().clone();
                    serde_json::to_string(&value).unwrap_or_else(|_| "[]".to_owned())
                })
                .map_err(|e| format!("could not register tabs: {e}"))?;
                globals
                    .set("__sikemuxTabs", tabs_fn)
                    .map_err(|e| format!("could not set tabs: {e}"))?;

                let profiles_json_str = profiles_for_js.clone();
                let profiles_fn = Function::new(ctx.clone(), move || -> String {
                    profiles_json_str.clone()
                })
                .map_err(|e| format!("could not register profiles: {e}"))?;
                globals
                    .set("__sikemuxProfiles", profiles_fn)
                    .map_err(|e| format!("could not set profiles: {e}"))?;

                let parse_url_fn = Function::new(
                    ctx.clone(),
                    move |input: String, base: Option<String>| -> String {
                        let envelope = match parse_url_for_js(&input, base.as_deref()) {
                            Ok(parts) => json!({ "ok": parts }),
                            Err(error) => json!({ "error": error }),
                        };
                        serde_json::to_string(&envelope)
                            .unwrap_or_else(|_| r#"{"error":"Invalid URL"}"#.into())
                    },
                )
                .map_err(|e| format!("could not register parseUrl: {e}"))?;
                globals
                    .set("__sikemuxParseUrl", parse_url_fn)
                    .map_err(|e| format!("could not set parseUrl: {e}"))?;

                let sleep_fn = Function::new(
                    ctx.clone(),
                    Async(move |ms: f64| async move {
                        let dur = Duration::from_millis((ms.clamp(0.0, 60_000.0)) as u64);
                        let left = deadline.saturating_duration_since(Instant::now());
                        tokio::time::sleep(dur.min(left)).await;
                    }),
                )
                .map_err(|e| format!("could not register sleep: {e}"))?;
                globals
                    .set("__sikemuxSleep", sleep_fn)
                    .map_err(|e| format!("could not set sleep: {e}"))?;

                let timers_wait = Arc::clone(&timers_for_js);
                let gate_wait = Arc::clone(&gate_for_timers);
                let wait_timer_fn = Function::new(
                    ctx.clone(),
                    Async(move |id: f64, ms: f64| {
                        let timers = Arc::clone(&timers_wait);
                        let gate = Arc::clone(&gate_wait);
                        async move {
                            let id = id as u64;
                            let dur = Duration::from_millis((ms.clamp(0.0, 60_000.0)) as u64);
                            let left = deadline.saturating_duration_since(Instant::now());
                            if left.is_zero() || gate.load(Ordering::Acquire) {
                                return false;
                            }
                            let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
                            timers.locked().insert(id, cancel_tx);
                            let fire = tokio::select! {
                                biased;
                                _ = cancel_rx => false,
                                () = tokio::time::sleep(dur.min(left)) => {
                                    Instant::now() < deadline && !gate.load(Ordering::Acquire)
                                }
                            };
                            timers.locked().remove(&id);
                            fire
                        }
                    }),
                )
                .map_err(|e| format!("could not register waitTimer: {e}"))?;
                globals
                    .set("__sikemuxWaitTimer", wait_timer_fn)
                    .map_err(|e| format!("could not set waitTimer: {e}"))?;

                let timers_cancel = Arc::clone(&timers_for_js);
                let cancel_timer_fn = Function::new(ctx.clone(), move |id: f64| {
                    if let Some(cancel) = timers_cancel.locked().remove(&(id as u64)) {
                        let _ = cancel.send(());
                    }
                })
                .map_err(|e| format!("could not register cancelTimer: {e}"))?;
                globals
                    .set("__sikemuxCancelTimer", cancel_timer_fn)
                    .map_err(|e| format!("could not set cancelTimer: {e}"))?;

                ctx.eval::<(), _>(DRIVE_SDK_JS)
                    .catch(&ctx)
                    .map_err(format_caught_error)?;

                let wrapped = format!(
                    "(async () => {{\n\
                     const __val = await (async () => {{\n{script_owned}\n}})();\n\
                     const __seen = new WeakSet();\n\
                     return JSON.stringify(__val === undefined ? null : __val, (_k, v) => {{\n\
                         if (typeof v === 'bigint' || typeof v === 'function' || typeof v === 'symbol') return String(v);\n\
                         if (typeof v === 'object' && v !== null) {{\n\
                             if (__seen.has(v)) return '[circular]';\n\
                             __seen.add(v);\n\
                         }}\n\
                         return v;\n\
                     }}) ?? 'null';\n\
                     }})()"
                );

                let promise: Promise<'_> = ctx
                    .eval(wrapped)
                    .catch(&ctx)
                    .map_err(format_caught_error)?;
                promise
                    .into_future::<String>()
                    .await
                    .catch(&ctx)
                    .map_err(format_caught_error)
            })
            .await;
        drop(tx);
        result
    };
    tokio::pin!(js_fut);

    let sleep_deadline = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
    tokio::pin!(sleep_deadline);

    let mut in_flight: Vec<InFlight<'_>> = Vec::new();
    let mut completed_steps: Vec<Value> = Vec::new();
    let mut next_step: usize = 0;
    let mut held_write: Option<HeldWrite> = None;

    let js_outcome: Option<Result<String, String>> = std::future::poll_fn(|cx| {
        if sleep_deadline.as_mut().poll(cx).is_ready() {
            timed_out.store(true, Ordering::Release);
            for (_, cancel) in timers.locked().drain() {
                let _ = cancel.send(());
            }
            return Poll::Ready(None);
        }

        // First poll JS so it can enqueue new primitive calls or finish.
        if let Poll::Ready(result) = js_fut.as_mut().poll(cx) {
            return Poll::Ready(Some(result));
        }

        // Refresh tab snapshot and drain any newly queued primitive calls.
        while let Poll::Ready(Some(req)) = req_rx.poll_recv(cx) {
            if !ALLOWED_DRIVE_METHODS.contains(&req.method.as_str()) {
                let _ = req.reply.send(Err(format!(
                    "{} is not available inside browser_drive",
                    req.method
                )));
                continue;
            }
            if next_step >= MAX_DRIVE_STEPS {
                let _ = req.reply.send(Err(format!(
                    "browser_drive exceeded the {MAX_DRIVE_STEPS}-step limit"
                )));
                continue;
            }
            let step = next_step;
            next_step += 1;
            let summary = step_summary(&req.method, &req.params);
            let tab_id = requested_tab(&req.params).map(str::to_owned);
            let future = backend.call(req.method.clone(), req.params);
            in_flight.push(InFlight {
                step,
                method: req.method,
                tab_id,
                summary,
                reply: Some(req.reply),
                future,
            });
        }

        // Poll all in-flight backend calls concurrently (enables Promise.all across tabs).
        let mut idx = 0;
        let mut any_progressed = false;
        while idx < in_flight.len() {
            let ready = {
                let Some(entry) = in_flight.get_mut(idx) else {
                    break;
                };
                match entry.future.as_mut().poll(cx) {
                    Poll::Ready(res) => Some(res),
                    Poll::Pending => None,
                }
            };
            if let Some(res) = ready {
                let mut entry = in_flight.swap_remove(idx);
                any_progressed = true;
                *tabs_snapshot.locked() = backend.tabs();
                match res {
                    Ok(value) => {
                        let resolved_tab = value
                            .get("tabId")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                            .or(entry.tab_id);
                        completed_steps.push(json!({
                            "step": entry.step,
                            "method": entry.method,
                            "tabId": resolved_tab,
                            "summary": entry.summary,
                        }));
                        if let Some(reply) = entry.reply.take() {
                            let _ = reply.send(Ok(value));
                        }
                    }
                    Err(error) if is_held_write_error(&error) => {
                        gate_aborted.store(true, Ordering::Release);
                        for (_, cancel) in timers.locked().drain() {
                            let _ = cancel.send(());
                        }
                        held_write = Some(HeldWrite {
                            step: entry.step,
                            method: entry.method,
                            tab_id: entry.tab_id,
                            summary: entry.summary,
                            error,
                        });
                        in_flight.clear();
                        return Poll::Ready(None);
                    }
                    Err(error) => {
                        if let Some(reply) = entry.reply.take() {
                            let _ = reply.send(Err(error));
                        }
                    }
                }
            } else {
                idx += 1;
            }
        }

        if any_progressed {
            // Re-poll JS immediately so resolved promises continue without waiting another tick.
            if let Poll::Ready(result) = js_fut.as_mut().poll(cx) {
                return Poll::Ready(Some(result));
            }
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    })
    .await;

    // Sort completedSteps by invocation order so parallel Promise.all steps read cleanly.
    completed_steps.sort_by_key(|entry| entry.get("step").and_then(Value::as_u64).unwrap_or(0));

    if let Some(held) = held_write {
        return json!({
            "ok": false,
            "heldAt": {
                "step": held.step,
                "method": held.method,
                "tabId": held.tab_id,
                "summary": held.summary,
            },
            "completedSteps": completed_steps,
            "error": held.error,
        });
    }

    if timed_out.load(Ordering::Acquire) {
        return json!({
            "ok": false,
            "error": format!("browser_drive script timed out after {} ms", timeout.as_millis()),
            "completedSteps": completed_steps,
        });
    }

    match js_outcome {
        Some(Ok(raw)) => {
            if raw.len() > MAX_SCRIPT_RESULT {
                let cut = (0..=MAX_SCRIPT_RESULT)
                    .rev()
                    .find(|at| raw.is_char_boundary(*at))
                    .unwrap_or(0);
                return json!({
                    "ok": true,
                    "result": raw.get(..cut),
                    "truncated": true,
                    "completedSteps": completed_steps,
                });
            }
            let result = serde_json::from_str::<Value>(&raw).unwrap_or(Value::String(raw));
            json!({
                "ok": true,
                "result": result,
                "completedSteps": completed_steps,
            })
        }
        Some(Err(error)) => json!({
            "ok": false,
            "error": error,
            "completedSteps": completed_steps,
        }),
        None => json!({
            "ok": false,
            "error": format!("browser_drive script timed out after {} ms", timeout.as_millis()),
            "completedSteps": completed_steps,
        }),
    }
}

fn parse_url_for_js(input: &str, base: Option<&str>) -> Result<Value, String> {
    let parsed = match base {
        Some(base_str) => {
            let base_url =
                url::Url::parse(base_str).map_err(|_| format!("Invalid base URL: {base_str}"))?;
            base_url
                .join(input)
                .map_err(|_| format!("Invalid URL: {input}"))?
        }
        None => url::Url::parse(input).map_err(|_| format!("Invalid URL: {input}"))?,
    };
    let origin = parsed.origin().ascii_serialization();
    let protocol = format!("{}:", parsed.scheme());
    let host_str = parsed.host_str().unwrap_or_default().to_owned();
    let port = parsed.port().map(|p| p.to_string()).unwrap_or_default();
    let host = if port.is_empty() {
        host_str.clone()
    } else {
        format!("{host_str}:{port}")
    };
    let search = parsed
        .query()
        .filter(|q| !q.is_empty())
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let hash = parsed
        .fragment()
        .filter(|f| !f.is_empty())
        .map(|f| format!("#{f}"))
        .unwrap_or_default();
    Ok(json!({
        "href": parsed.as_str(),
        "origin": origin,
        "protocol": protocol,
        "username": parsed.username(),
        "password": parsed.password().unwrap_or_default(),
        "host": host,
        "hostname": host_str,
        "port": port,
        "pathname": parsed.path(),
        "search": search,
        "hash": hash,
    }))
}

fn format_caught_error(error: CaughtError<'_>) -> String {
    match error {
        CaughtError::Exception(exc) => {
            let msg = exc.message().unwrap_or_default();
            if msg.is_empty() {
                format!("{exc:?}")
            } else {
                msg
            }
        }
        CaughtError::Value(val) => format!("{val:?}"),
        CaughtError::Error(err) => err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct MockBackend {
        tabs: Mutex<Vec<Value>>,
        in_flight_now: AtomicUsize,
        max_concurrent: AtomicUsize,
        call_delay: Duration,
        hold_on_text: Option<String>,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                tabs: Mutex::new(vec![
                    json!({ "id": "tab-1", "title": "One", "url": "https://one.test", "active": true, "yours": false }),
                    json!({ "id": "tab-2", "title": "Two", "url": "https://two.test", "active": false, "yours": false }),
                ]),
                in_flight_now: AtomicUsize::new(0),
                max_concurrent: AtomicUsize::new(0),
                call_delay: Duration::from_millis(40),
                hold_on_text: None,
            }
        }
    }

    impl DriveBackend for MockBackend {
        fn call<'a>(
            &'a self,
            method: String,
            params: Value,
        ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + 'a>> {
            Box::pin(async move {
                let current = self.in_flight_now.fetch_add(1, Ordering::AcqRel) + 1;
                self.max_concurrent.fetch_max(current, Ordering::AcqRel);
                if !self.call_delay.is_zero() {
                    tokio::time::sleep(self.call_delay).await;
                }
                self.in_flight_now.fetch_sub(1, Ordering::AcqRel);

                if let Some(needle) = &self.hold_on_text {
                    if params.get("text").and_then(Value::as_str) == Some(needle.as_str()) {
                        return Err(
                            "Waits for the person's yes: `POST https://prod.test/deploy` changes prod.test, a production place."
                                .to_owned(),
                        );
                    }
                }
                match method.as_str() {
                    "browser.navigate" => {
                        let url = params
                            .get("url")
                            .and_then(Value::as_str)
                            .unwrap_or("https://new.test");
                        let tab_id = if params.get("newTab").and_then(Value::as_bool) == Some(true)
                        {
                            let mut tabs = self.tabs.locked();
                            let id = format!("tab-{}", tabs.len() + 1);
                            tabs.push(json!({ "id": id, "title": url, "url": url, "active": false, "yours": false }));
                            id
                        } else {
                            params
                                .get("tabId")
                                .and_then(Value::as_str)
                                .unwrap_or("tab-1")
                                .to_owned()
                        };
                        Ok(json!({ "tabId": tab_id, "url": url, "title": url, "loading": false }))
                    }
                    "browser.state" => {
                        let tab_id = params
                            .get("tabId")
                            .and_then(Value::as_str)
                            .unwrap_or("tab-1");
                        Ok(
                            json!({ "tabId": tab_id, "text": format!("body of {tab_id}"), "elements": "" }),
                        )
                    }
                    "browser.click" => {
                        if params.get("text").and_then(Value::as_str) == Some("Missing") {
                            return Err("nothing shows or is labelled \"Missing\"".into());
                        }
                        let tab_id = params
                            .get("tabId")
                            .and_then(Value::as_str)
                            .unwrap_or("tab-1");
                        Ok(json!({ "tabId": tab_id, "action": "clicked" }))
                    }
                    "browser.type" => {
                        let tab_id = params
                            .get("tabId")
                            .and_then(Value::as_str)
                            .unwrap_or("tab-1");
                        Ok(json!({
                            "tabId": tab_id,
                            "typed": 5,
                            "selector": params.get("selector").cloned().unwrap_or(Value::Null),
                            "field": params.get("field").cloned().unwrap_or(Value::Null),
                            "text": params.get("text").cloned().unwrap_or(Value::Null),
                            "replace": params.get("replace").cloned().unwrap_or(Value::Null),
                        }))
                    }
                    "browser.select" => Ok(json!({
                        "tabId": params.get("tabId").and_then(Value::as_str).unwrap_or("tab-1"),
                        "selected": params.get("value").cloned().unwrap_or(Value::Null),
                        "field": params.get("field").cloned().unwrap_or(Value::Null),
                    })),
                    "browser.upload" => Ok(json!({
                        "tabId": params.get("tabId").and_then(Value::as_str).unwrap_or("tab-1"),
                        "uploaded": params.get("paths").cloned().unwrap_or_else(|| json!([])),
                        "selector": params.get("selector").cloned().unwrap_or(Value::Null),
                    })),
                    "browser.dialog" => {
                        if self.hold_on_text.as_deref() == Some("dialog") {
                            return Err(
                                "Waits for the person's yes: `POST https://prod.test/confirm` changes prod.test, a production place."
                                    .to_owned(),
                            );
                        }
                        Ok(json!({
                            "tabId": params.get("tabId").and_then(Value::as_str).unwrap_or("tab-1"),
                            "accepted": params.get("accept").cloned().unwrap_or(Value::Null),
                            "text": params.get("text").cloned().unwrap_or(Value::Null),
                        }))
                    }
                    "browser.download" => Ok(json!({
                        "tabId": params.get("tabId").and_then(Value::as_str).unwrap_or("tab-1"),
                        "path": "/tmp/desk-downloads/photo.jpg",
                        "mimeType": "image/jpeg",
                        "bytes": 4200,
                        "url": params.get("url").cloned().unwrap_or(Value::Null),
                        "filename": params.get("filename").cloned().unwrap_or(Value::Null),
                    })),
                    "browser.evaluate" => Ok(json!({
                        "tabId": params.get("tabId").and_then(Value::as_str).unwrap_or("tab-1"),
                        "result": params.get("script").cloned().unwrap_or(Value::Null),
                        "timeoutMs": params.get("timeoutMs").cloned().unwrap_or(Value::Null),
                    })),
                    _ => Ok(json!({ "ok": true })),
                }
            })
        }

        fn tabs(&self) -> Value {
            Value::Array(self.tabs.locked().clone())
        }
    }

    #[tokio::test]
    async fn browser_drive_runs_promise_all_across_multiple_tabs_in_parallel() {
        let backend = MockBackend::new();
        let script = r#"
            const [t1, t2, t3] = await Promise.all([
                browser.open("https://a.test", { background: true }),
                browser.open("https://b.test", { background: true }),
                browser.open("https://c.test", { background: true }),
            ]);
            const states = await Promise.all([t1.state(), t2.state(), t3.state()]);
            return states.map(s => ({ id: s.tabId, text: s.text }));
        "#;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(true), "unexpected output: {out}");
        assert!(
            backend.max_concurrent.load(Ordering::Acquire) >= 2,
            "expected concurrent execution across tabs, max_concurrent was {}",
            backend.max_concurrent.load(Ordering::Acquire)
        );
        let arr = out["result"].as_array().expect("result array");
        assert_eq!(arr.len(), 3);
        assert_eq!(out["completedSteps"].as_array().map(Vec::len), Some(6));
    }

    #[tokio::test]
    async fn browser_drive_sandbox_has_no_host_globals() {
        let backend = MockBackend::new();
        let script = r#"
            return {
                hasRequire: typeof require !== "undefined",
                hasProcess: typeof process !== "undefined",
                hasFetch: typeof fetch !== "undefined",
                hasWindow: typeof window !== "undefined",
                hasDocument: typeof document !== "undefined",
                hasBrowser: typeof browser === "object" && typeof browser.open === "function",
                tabsCount: browser.tabs().length,
            };
        "#;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(true), "unexpected output: {out}");
        assert_eq!(
            out["result"],
            json!({
                "hasRequire": false,
                "hasProcess": false,
                "hasFetch": false,
                "hasWindow": false,
                "hasDocument": false,
                "hasBrowser": true,
                "tabsCount": 2,
            })
        );
    }

    #[tokio::test]
    async fn browser_drive_enforces_timeout_on_infinite_loop() {
        let backend = MockBackend::new();
        let started = Instant::now();
        let out = run_drive(&backend, "while (true) {}", Duration::from_millis(120)).await;
        assert_eq!(out["ok"], json!(false));
        let err = out["error"].as_str().unwrap_or_default();
        assert!(
            err.contains("timed out"),
            "expected timeout error, got: {err}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timeout took too long: {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn browser_drive_fails_fast_on_held_write_gate_with_step_ledger() {
        let mut backend = MockBackend::new();
        backend.call_delay = Duration::ZERO;
        backend.hold_on_text = Some("Deploy".into());
        let script = r#"
            const tab = browser.tab("tab-1");
            await tab.type("Email", "a@b.test");
            // Even wrapped in try/catch or optional, a held write gate must fail fast and never be swallowed.
            try {
                await tab.click("Deploy", { optional: true });
            } catch {}
            await tab.click("AfterDeploy");
            return "should never reach";
        "#;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(false), "expected fail-fast: {out}");
        assert_eq!(out["heldAt"]["method"], json!("browser.click"));
        assert_eq!(out["heldAt"]["tabId"], json!("tab-1"));
        assert_eq!(out["heldAt"]["step"], json!(1));
        assert!(out["error"]
            .as_str()
            .unwrap_or_default()
            .starts_with("Waits for the person"));
        let completed = out["completedSteps"].as_array().expect("completedSteps");
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0]["method"], json!("browser.type"));
    }

    #[tokio::test]
    async fn browser_drive_optional_swallows_normal_errors() {
        let mut backend = MockBackend::new();
        backend.call_delay = Duration::ZERO;
        let script = r#"
            const tab = browser.tab("tab-1");
            const skipped = await tab.click("Missing", { optional: true });
            const st = await tab.state();
            return { skipped, text: st.text };
        "#;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(true), "unexpected output: {out}");
        assert_eq!(out["result"]["skipped"]["skipped"], json!(true));
        assert_eq!(out["result"]["text"], json!("body of tab-1"));
    }

    #[tokio::test]
    async fn browser_drive_enforces_memory_limit() {
        let backend = MockBackend::new();
        let script = r#"
            const chunks = [];
            for (let i = 0; i < 200; i++) {
                chunks.push("x".repeat(1024 * 1024));
            }
            return chunks.length;
        "#;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(false), "expected OOM failure: {out}");
        assert!(
            out["error"].as_str().is_some_and(|e| !e.is_empty()),
            "expected non-empty error: {out}"
        );
    }

    #[tokio::test]
    async fn browser_drive_tab_exposes_select_upload_dialog_and_download() {
        let mut backend = MockBackend::new();
        backend.call_delay = Duration::ZERO;
        let script = r##"
            const tab = browser.tab("tab-1");
            const sel = await tab.select("#state", "Karnataka");
            const up = await tab.upload('input[type="file"]', ["/tmp/pan.pdf"]);
            const dlg = await tab.dialog("accept", "confirmed");
            const dl = await tab.download("https://lh3.googleusercontent.com/abc", { filename: "photo.jpg" });
            return { sel, up, dlg, dl };
        "##;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(true), "unexpected output: {out}");
        assert_eq!(out["result"]["sel"]["selected"], json!("Karnataka"));
        assert_eq!(out["result"]["sel"]["field"], json!("#state"));
        assert_eq!(out["result"]["up"]["uploaded"], json!(["/tmp/pan.pdf"]));
        assert_eq!(
            out["result"]["up"]["selector"],
            json!("input[type=\"file\"]")
        );
        assert_eq!(out["result"]["dlg"]["accepted"], json!(true));
        assert_eq!(out["result"]["dlg"]["text"], json!("confirmed"));
        assert_eq!(
            out["result"]["dl"]["path"],
            json!("/tmp/desk-downloads/photo.jpg")
        );
        assert_eq!(out["result"]["dl"]["mimeType"], json!("image/jpeg"));
        assert_eq!(out["result"]["dl"]["bytes"], json!(4200));
        assert_eq!(out["result"]["dl"]["filename"], json!("photo.jpg"));
    }

    #[tokio::test]
    async fn browser_drive_dialog_respects_fail_fast_write_gate() {
        let mut backend = MockBackend::new();
        backend.call_delay = Duration::ZERO;
        backend.hold_on_text = Some("dialog".into());
        let script = r#"
            const tab = browser.tab("tab-1");
            try {
                await tab.dialog("accept", { optional: true });
            } catch {}
            return "unreachable";
        "#;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(false), "expected held gate: {out}");
        assert_eq!(out["heldAt"]["method"], json!("browser.dialog"));
        assert!(out["error"]
            .as_str()
            .unwrap_or_default()
            .starts_with("Waits for the person"));
    }

    #[tokio::test]
    async fn browser_drive_type_merges_target_object_with_text_and_opts() {
        let mut backend = MockBackend::new();
        backend.call_delay = Duration::ZERO;
        let script = r##"
            const tab = browser.tab("tab-1");
            const a = await tab.type({ selector: "#q" }, "hello");
            const b = await tab.type({ selector: "#q" }, "world", { replace: true });
            const c = await tab.type({ selector: "#q", text: "inline" });
            return { a, b, c };
        "##;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(true), "unexpected output: {out}");
        assert_eq!(out["result"]["a"]["selector"], json!("#q"));
        assert_eq!(out["result"]["a"]["text"], json!("hello"));
        assert_eq!(out["result"]["b"]["selector"], json!("#q"));
        assert_eq!(out["result"]["b"]["text"], json!("world"));
        assert_eq!(out["result"]["b"]["replace"], json!(true));
        assert_eq!(out["result"]["c"]["selector"], json!("#q"));
        assert_eq!(out["result"]["c"]["text"], json!("inline"));
    }

    #[tokio::test]
    async fn browser_drive_fill_passes_vault_ref_with_replace_true() {
        let mut backend = MockBackend::new();
        backend.call_delay = Duration::ZERO;
        let script = r##"
            const tab = browser.tab("tab-1");
            const filled = await tab.fill("Card number", "vault:vlt_card.card_number");
            const uploaded = await tab.upload('input[type="file"]', "vault:vlt_doc");
            return { filled, uploaded };
        "##;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(true), "unexpected output: {out}");
        assert_eq!(
            out["result"]["filled"]["text"],
            json!("vault:vlt_card.card_number")
        );
        assert_eq!(out["result"]["filled"]["replace"], json!(true));
        assert_eq!(
            out["result"]["uploaded"]["uploaded"],
            json!(["vault:vlt_doc"])
        );
    }

    #[tokio::test]
    async fn browser_drive_provides_url_and_url_search_params() {
        let mut backend = MockBackend::new();
        backend.call_delay = Duration::ZERO;
        let script = r#"
            const u = new URL("/flights?from=SFO&to=JFK#top", "http://t1.localhost:8000/home");
            u.searchParams.set("to", "BOS");
            u.searchParams.append("cabin", "economy plus");
            const rel = new URL("../deals", u.href);
            const params = new URLSearchParams({ a: "1", b: "two words" });
            params.sort();
            return {
                href: u.href,
                origin: u.origin,
                hostname: u.hostname,
                port: u.port,
                pathname: u.pathname,
                search: u.search,
                hash: u.hash,
                from: u.searchParams.get("from"),
                to: u.searchParams.get("to"),
                cabin: u.searchParams.get("cabin"),
                relHref: rel.href,
                paramsStr: params.toString(),
                canParseGood: URL.canParse("https://example.test/x"),
                canParseBad: URL.canParse("not a url"),
                parsedNull: URL.parse(":::"),
            };
        "#;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(true), "unexpected output: {out}");
        assert_eq!(
            out["result"]["href"],
            json!("http://t1.localhost:8000/flights?from=SFO&to=BOS&cabin=economy+plus#top")
        );
        assert_eq!(out["result"]["origin"], json!("http://t1.localhost:8000"));
        assert_eq!(out["result"]["hostname"], json!("t1.localhost"));
        assert_eq!(out["result"]["port"], json!("8000"));
        assert_eq!(out["result"]["from"], json!("SFO"));
        assert_eq!(out["result"]["to"], json!("BOS"));
        assert_eq!(out["result"]["cabin"], json!("economy plus"));
        assert_eq!(
            out["result"]["relHref"],
            json!("http://t1.localhost:8000/deals")
        );
        assert_eq!(out["result"]["paramsStr"], json!("a=1&b=two+words"));
        assert_eq!(out["result"]["canParseGood"], json!(true));
        assert_eq!(out["result"]["canParseBad"], json!(false));
        assert_eq!(out["result"]["parsedNull"], Value::Null);
    }

    #[tokio::test]
    async fn browser_drive_provides_cancellable_bounded_set_timeout() {
        let mut backend = MockBackend::new();
        backend.call_delay = Duration::ZERO;
        let script = r#"
            const events = [];
            const cancelled = setTimeout(() => events.push("cancelled"), 20);
            clearTimeout(cancelled);
            const result = await new Promise((resolve) => {
                setTimeout((a, b) => {
                    events.push("fired");
                    resolve(a + b);
                }, 15, 20, 22);
            });
            await sleep(30);
            return { result, events };
        "#;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(true), "unexpected output: {out}");
        assert_eq!(out["result"]["result"], json!(42));
        assert_eq!(out["result"]["events"], json!(["fired"]));

        let started = Instant::now();
        let timed = run_drive(
            &backend,
            "await new Promise((r) => setTimeout(r, 10000)); return 'late';",
            Duration::from_millis(120),
        )
        .await;
        assert_eq!(timed["ok"], json!(false));
        assert!(timed["error"]
            .as_str()
            .unwrap_or_default()
            .contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn browser_drive_supports_positional_and_object_eval_type_and_clear_error_hints() {
        let mut backend = MockBackend::new();
        backend.call_delay = Duration::ZERO;
        let script = r##"
            const tab = browser.tab("tab-1");
            const e1 = await tab.eval("document.title");
            const e2 = await tab.eval({ script: "location.href", timeoutMs: 3000 });
            const t1 = await tab.type("Where from?", "SFO");
            const t2 = await tab.type("Price", 500, { replace: true });
            const t3 = await tab.type(3, "NYC");
            const t4 = await tab.type({ selector: "#q" }, 94107);
            const errors = {};
            for (const [key, fn] of [
                ["valueKey", () => tab.type({ selector: "#q", value: "SFO" })],
                ["missingText", () => tab.type({ selector: "#q" })],
                ["missingScript", () => tab.eval({})],
                ["unknownNavigate", () => tab.navigate("https://a.test")],
                ["unknownFoo", () => tab.foo()],
            ]) {
                try {
                    await fn();
                } catch (err) {
                    errors[key] = err.message;
                }
            }
            return { e1, e2, t1, t2, t3, t4, errors };
        "##;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(true), "unexpected output: {out}");
        assert_eq!(out["result"]["e1"]["result"], json!("document.title"));
        assert_eq!(out["result"]["e2"]["result"], json!("location.href"));
        assert_eq!(out["result"]["e2"]["timeoutMs"], json!(3000));
        assert_eq!(out["result"]["t1"]["field"], json!("Where from?"));
        assert_eq!(out["result"]["t1"]["text"], json!("SFO"));
        assert_eq!(out["result"]["t2"]["field"], json!("Price"));
        assert_eq!(out["result"]["t2"]["text"], json!("500"));
        assert_eq!(out["result"]["t2"]["replace"], json!(true));
        assert_eq!(out["result"]["t3"]["text"], json!("NYC"));
        assert_eq!(out["result"]["t4"]["selector"], json!("#q"));
        assert_eq!(out["result"]["t4"]["text"], json!("94107"));
        assert!(out["result"]["errors"]["valueKey"]
            .as_str()
            .unwrap_or_default()
            .contains("use \"text\", not \"value\""));
        assert!(out["result"]["errors"]["missingText"]
            .as_str()
            .unwrap_or_default()
            .contains("tab.type(target, text, opts?)"));
        assert!(out["result"]["errors"]["missingScript"]
            .as_str()
            .unwrap_or_default()
            .contains("tab.eval(script, opts?)"));
        assert!(out["result"]["errors"]["unknownNavigate"]
            .as_str()
            .unwrap_or_default()
            .contains("use tab.goto(url, opts?)"));
        assert!(out["result"]["errors"]["unknownFoo"]
            .as_str()
            .unwrap_or_default()
            .contains("Valid tab methods:"));
    }
}
