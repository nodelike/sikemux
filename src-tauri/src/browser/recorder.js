// Runs before a page's own scripts, on every document, so the calls a page
// makes and what it logs are already recorded by the time an agent asks.
// Reading the DOM cannot answer "did that button reach the server, and what
// came back", nor "did the page throw".
(() => {
  if (window.__sikemuxNet) return;
  const MAX_ENTRIES = 120;
  const MAX_BODY = 4000;
  // A body is only worth keeping when it is text an agent can read.
  const TEXTUAL =
    /^(?:text\/|application\/(?:json|xml|javascript|x-www-form-urlencoded|[\w.+-]*\+json))/i;

  const entries = [];
  let sequence = 0;

  const at = () => Math.round(performance.now());
  const clip = (text) =>
    text.length > MAX_BODY ? `${text.slice(0, MAX_BODY)}…` : text;
  const add = (entry) => {
    entries.push(entry);
    if (entries.length > MAX_ENTRIES) entries.shift();
    return entry;
  };
  const begin = (kind, method, url) =>
    add({
      id: ++sequence,
      kind,
      method: String(method || "GET").toUpperCase(),
      url: String(url),
      startedAt: at(),
      status: null,
      statusText: "",
      contentType: "",
      durationMs: null,
      requestBody: null,
      body: null,
      error: null,
    });
  const finish = (entry) => {
    entry.durationMs = at() - entry.startedAt;
  };
  const describeRequest = (body) => {
    if (body == null) return null;
    if (typeof body === "string") return clip(body);
    if (body instanceof URLSearchParams) return clip(body.toString());
    return `[${body.constructor ? body.constructor.name : typeof body}]`;
  };

  const nativeFetch = window.fetch;
  if (typeof nativeFetch === "function") {
    window.fetch = function (input, init) {
      const request =
        typeof input === "object" && input !== null && "url" in input
          ? input
          : null;
      const entry = begin(
        "fetch",
        (init && init.method) || (request && request.method) || "GET",
        (request && request.url) || input,
      );
      entry.requestBody = describeRequest(init && init.body);
      return nativeFetch.apply(this, arguments).then(
        (response) => {
          finish(entry);
          entry.status = response.status;
          entry.statusText = response.statusText || "";
          entry.contentType = response.headers.get("content-type") || "";
          // Read a copy in the background: awaiting it here would hold
          // up the page's own handler, and a binary stream would be
          // buffered for nothing.
          if (TEXTUAL.test(entry.contentType)) {
            response
              .clone()
              .text()
              .then((text) => {
                entry.body = clip(text);
              })
              .catch(() => {});
          }
          return response;
        },
        (error) => {
          finish(entry);
          entry.error = String((error && error.message) || error);
          throw error;
        },
      );
    };
  }

  const open = XMLHttpRequest.prototype.open;
  const send = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function (method, url) {
    this.__sikemuxCall = { method, url };
    return open.apply(this, arguments);
  };
  XMLHttpRequest.prototype.send = function (body) {
    const call = this.__sikemuxCall;
    if (call) {
      const entry = begin("xhr", call.method, call.url);
      entry.requestBody = describeRequest(body);
      this.addEventListener("loadend", () => {
        finish(entry);
        entry.status = this.status;
        entry.statusText = this.statusText || "";
        try {
          entry.contentType = this.getResponseHeader("content-type") || "";
        } catch {
          entry.contentType = "";
        }
        if (this.status === 0) entry.error = "the request did not complete";
        try {
          if (this.responseType === "" || this.responseType === "text")
            entry.body = clip(String(this.responseText));
          else if (this.responseType === "json")
            entry.body = clip(JSON.stringify(this.response));
        } catch {
          entry.body = null;
        }
      });
    }
    return send.apply(this, arguments);
  };

  window.__sikemuxNet = { entries: () => entries };
})();

(() => {
  if (window.__sikemuxConsole) return;
  const MAX_ENTRIES = 200;
  const MAX_TEXT = 2000;
  const entries = [];

  const clip = (text) =>
    text.length > MAX_TEXT ? `${text.slice(0, MAX_TEXT)}…` : text;
  const describe = (value) => {
    if (typeof value === "string") return value;
    // WebKit's stack lists only the frames, never the message.
    if (value instanceof Error)
      return [`${value.name}: ${value.message}`, value.stack]
        .filter(Boolean)
        .join("\n");
    if (typeof Node === "function" && value instanceof Node)
      return `<${(value.nodeName || "node").toLowerCase()}>`;
    try {
      const json = JSON.stringify(value);
      return json === undefined ? String(value) : json;
    } catch {
      return String(value);
    }
  };
  const add = (level, parts) => {
    entries.push({
      level,
      at: Math.round(performance.now()),
      text: clip(parts.map(describe).join(" ")),
    });
    if (entries.length > MAX_ENTRIES) entries.shift();
  };

  for (const level of ["log", "info", "warn", "error", "debug"]) {
    const original = console[level];
    if (typeof original !== "function") continue;
    console[level] = function (...parts) {
      add(level, parts);
      return original.apply(this, parts);
    };
  }
  window.addEventListener("error", (event) => {
    if (event.error || event.message)
      add("uncaught", [event.error || event.message]);
  });
  window.addEventListener("unhandledrejection", (event) => {
    add("unhandled rejection", [event.reason]);
  });

  window.__sikemuxConsole = { entries: () => entries };
})();
