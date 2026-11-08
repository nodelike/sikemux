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
  const headerOf = (headers, name) => {
    if (!headers) return null;
    try {
      if (typeof headers.get === "function") return headers.get(name);
      const pairs = Array.isArray(headers) ? headers : Object.entries(headers);
      const pair = pairs.find(([key]) => String(key).toLowerCase() === name);
      return pair ? String(pair[1]) : null;
    } catch {
      return null;
    }
  };
  // Framework data requests look like any other fetch, so name the common ones.
  const frameworkOf = (url, headers) => {
    if (headerOf(headers, "next-action")) return "next server action";
    if (/[?&]_rsc=/.test(url) || headerOf(headers, "rsc") === "1")
      return "next rsc";
    if (/\/_next\/data\//.test(url)) return "next data";
    return null;
  };
  const label = (entry, headers) => {
    const framework = frameworkOf(entry.url, headers);
    if (framework) entry.framework = framework;
  };
  const failure = (error) =>
    error && error.name && error.message
      ? `${error.name}: ${error.message}`
      : String((error && error.message) || error);
  const describeRequest = (body) => {
    if (body == null) return null;
    if (typeof body === "string") return clip(body);
    if (body instanceof URLSearchParams) return clip(body.toString());
    return `[${body.constructor ? body.constructor.name : typeof body}]`;
  };

  // While an agent's script runs, a write it makes to a site stops here, so the
  // person is asked first. A frame of that script has no address on the stack,
  // unlike the page's own, so the page's requests in those moments go on.
  const HELD = "Sikemux held this write for the person";
  let watch = null;
  const localHost = (host) =>
    host === "localhost" ||
    host === "[::1]" ||
    host.endsWith(".local") ||
    host.startsWith("127.");
  const fromAgent = (ownFrames) =>
    String(new Error().stack)
      .split("\n")
      .slice(ownFrames + 2)
      .some((frame) => frame.endsWith("@"));
  const held = (method, target, ownFrames = 1) => {
    if (!watch) return false;
    const verb = String(method || "GET").toUpperCase();
    if (verb === "GET" || verb === "HEAD" || verb === "OPTIONS") return false;
    if (!fromAgent(ownFrames)) return false;
    let url;
    try {
      url = new URL(String(target), document.baseURI);
    } catch {
      return false;
    }
    if (
      !/^https?:$/.test(url.protocol) ||
      localHost(url.hostname) ||
      watch.allowed.includes(url.origin)
    )
      return false;
    if (!watch.write)
      watch.write = { method: verb, origin: url.origin, path: url.pathname };
    return true;
  };
  const formHeld = (form, submitter) => {
    const method =
      (submitter && submitter.getAttribute("formmethod")) ||
      form.getAttribute("method") ||
      "get";
    if (method.toLowerCase() === "dialog") return false;
    return held(
      method,
      (submitter && submitter.getAttribute("formaction")) ||
        form.getAttribute("action") ||
        location.href,
      2,
    );
  };

  const nativeFetch = window.fetch;
  if (typeof nativeFetch === "function") {
    window.fetch = function (input, init) {
      const request =
        typeof input === "object" && input !== null && "url" in input
          ? input
          : null;
      const method =
        (init && init.method) || (request && request.method) || "GET";
      const address = (request && request.url) || input;
      if (held(method, address)) return Promise.reject(new TypeError(HELD));
      const entry = begin("fetch", method, address);
      entry.requestBody = describeRequest(init && init.body);
      label(entry, (init && init.headers) || (request && request.headers));
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
          entry.error = failure(error);
          throw error;
        },
      );
    };
  }

  const open = XMLHttpRequest.prototype.open;
  const send = XMLHttpRequest.prototype.send;
  const setRequestHeader = XMLHttpRequest.prototype.setRequestHeader;
  XMLHttpRequest.prototype.open = function (method, url) {
    this.__sikemuxCall = { method, url, headers: {} };
    return open.apply(this, arguments);
  };
  XMLHttpRequest.prototype.setRequestHeader = function (name, value) {
    if (this.__sikemuxCall) this.__sikemuxCall.headers[name] = value;
    return setRequestHeader.apply(this, arguments);
  };
  XMLHttpRequest.prototype.send = function (body) {
    const call = this.__sikemuxCall;
    if (call && held(call.method, call.url)) throw new TypeError(HELD);
    if (call) {
      const entry = begin("xhr", call.method, call.url);
      entry.requestBody = describeRequest(body);
      label(entry, call.headers);
      let ended = "the request did not complete";
      this.addEventListener("error", () => {
        ended = "network error";
      });
      this.addEventListener("abort", () => {
        ended = "aborted";
      });
      this.addEventListener("timeout", () => {
        ended = "timed out";
      });
      this.addEventListener("loadend", () => {
        finish(entry);
        entry.status = this.status;
        entry.statusText = this.statusText || "";
        try {
          entry.contentType = this.getResponseHeader("content-type") || "";
        } catch {
          entry.contentType = "";
        }
        if (this.status === 0) entry.error = ended;
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

  if (typeof navigator.sendBeacon === "function") {
    const sendBeacon = Navigator.prototype.sendBeacon;
    Navigator.prototype.sendBeacon = function (url) {
      if (held("POST", url)) throw new TypeError(HELD);
      return sendBeacon.apply(this, arguments);
    };
  }
  const submit = HTMLFormElement.prototype.submit;
  HTMLFormElement.prototype.submit = function () {
    if (formHeld(this, null)) throw new TypeError(HELD);
    return submit.apply(this, arguments);
  };
  const requestSubmit = HTMLFormElement.prototype.requestSubmit;
  if (typeof requestSubmit === "function") {
    HTMLFormElement.prototype.requestSubmit = function (submitter) {
      if (formHeld(this, submitter || null)) throw new TypeError(HELD);
      return requestSubmit.apply(this, arguments);
    };
  }
  window.addEventListener(
    "submit",
    (event) => {
      if (formHeld(event.target, event.submitter || null)) {
        event.preventDefault();
        event.stopImmediatePropagation();
      }
    },
    true,
  );

  window.__sikemuxNet = {
    entries: () => entries,
    watch: (allowed) => {
      watch = { allowed, write: null };
    },
    release: () => {
      const write = watch && watch.write;
      watch = null;
      return write;
    },
  };
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
    // WebKit hides errors from another origin or script world behind this
    // bare message, and the agent's own tools raise it after every action.
    const muted =
      !event.error && !event.filename && event.message === "Script error.";
    if (muted) return;
    if (event.error || event.message)
      add("uncaught", [event.error || event.message]);
  });
  window.addEventListener("unhandledrejection", (event) => {
    add("unhandled rejection", [event.reason]);
  });

  window.__sikemuxConsole = { entries: () => entries };
})();
