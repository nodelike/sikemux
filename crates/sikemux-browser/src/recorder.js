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
  // person is asked first. Control methods require a per-session token held only
  // in Rust, timers scheduled from agent scripts remain tainted after release,
  // and dynamic same-origin/about:blank iframes are patched on access.
  const GATE_TOKEN = "__SIKEMUX_GATE_TOKEN__";
  const checkToken = (token) =>
    typeof token === "string" && token === GATE_TOKEN;
  const HELD = "Sikemux held this write for the person";
  let watch = null;
  let lastAllowed = [];
  let agentDepth = 0;
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
  const isAgentContext = (ownFrames) =>
    agentDepth > 0 || fromAgent(ownFrames + 1);
  const siteWrite = (method, target) => {
    const verb = String(method || "GET").toUpperCase();
    if (verb === "GET" || verb === "HEAD" || verb === "OPTIONS") return null;
    let url;
    try {
      url = new URL(String(target), document.baseURI);
    } catch {
      return null;
    }
    if (!/^https?:$/.test(url.protocol) || localHost(url.hostname)) return null;
    return { method: verb, origin: url.origin, path: url.pathname, query: url.search };
  };
  const wsWrite = (target) => {
    let url;
    try {
      url = new URL(String(target), document.baseURI);
    } catch {
      return null;
    }
    if (!/^wss?:$/.test(url.protocol) || localHost(url.hostname)) return null;
    return {
      method: "WEBSOCKET",
      origin: url.origin,
      path: url.pathname,
      query: url.search,
    };
  };
  const held = (method, target, ownFrames = 1) => {
    if (!isAgentContext(ownFrames)) return false;
    const write =
      method === "WEBSOCKET" ? wsWrite(target) : siteWrite(method, target);
    if (!write) return false;
    if (watch) {
      if (watch.allowed.includes(write.origin)) return false;
      if (!watch.write) watch.write = write;
      return true;
    }
    if (agentDepth > 0 && !lastAllowed.includes(write.origin)) return true;
    return false;
  };

  // While an agent's click or key plays out, a write the page's own scripts
  // send waits here until the person's gate answers it.
  const HOLD_MS = 60000;
  let holding = null;
  let holdEnds = null;
  let lastWait = 0;
  const waits = new Map();
  const answer = (ids, go) => {
    for (const id of ids) {
      const resolve = waits.get(id);
      waits.delete(id);
      if (resolve) resolve(go);
    }
  };
  const stopHolding = () => {
    clearTimeout(holdEnds);
    const writes = holding || [];
    holding = null;
    return writes;
  };
  const waitForPerson = (method, target) => {
    if (!holding) return null;
    const write = siteWrite(method, target);
    if (!write) return null;
    const id = ++lastWait;
    holding.push({ id, ...write });
    return new Promise((resolve) => waits.set(id, resolve));
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

  const patchedWindows = new WeakSet();

  const wrapTainted = (callback, win) => {
    if (typeof callback === "function") {
      return function () {
        agentDepth += 1;
        try {
          return callback.apply(this, arguments);
        } finally {
          agentDepth -= 1;
        }
      };
    }
    if (typeof callback === "string") {
      const code = callback;
      return function () {
        agentDepth += 1;
        try {
          return win.Function(code).call(this);
        } finally {
          agentDepth -= 1;
        }
      };
    }
    return callback;
  };

  const installIn = (win) => {
    if (!win || patchedWindows.has(win)) return;
    patchedWindows.add(win);

    const nativeFetch = win.fetch;
    if (typeof nativeFetch === "function") {
      win.fetch = function (input, init) {
        const request =
          typeof input === "object" && input !== null && "url" in input
            ? input
            : null;
        const method =
          (init && init.method) || (request && request.method) || "GET";
        const address = (request && request.url) || input;
        if (held(method, address)) return Promise.reject(new TypeError(HELD));
        const waiting = waitForPerson(method, address);
        if (!waiting)
          return recordedFetch(this, arguments, method, address, request, init);
        return waiting.then((go) =>
          go
            ? recordedFetch(this, arguments, method, address, request, init)
            : Promise.reject(new TypeError(HELD)),
        );
      };
      const recordedFetch = (self, args, method, address, request, init) => {
        const entry = begin("fetch", method, address);
        entry.requestBody = describeRequest(init && init.body);
        label(entry, (init && init.headers) || (request && request.headers));
        return nativeFetch.apply(self, args).then(
          (response) => {
            finish(entry);
            entry.status = response.status;
            entry.statusText = response.statusText || "";
            entry.contentType = response.headers.get("content-type") || "";
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

    if (win.XMLHttpRequest && win.XMLHttpRequest.prototype) {
      const open = win.XMLHttpRequest.prototype.open;
      const send = win.XMLHttpRequest.prototype.send;
      const setRequestHeader = win.XMLHttpRequest.prototype.setRequestHeader;
      const abort = win.XMLHttpRequest.prototype.abort;
      win.XMLHttpRequest.prototype.open = function (method, url, async) {
        delete this.readyState;
        this.__sikemuxCall = {
          method,
          url,
          headers: {},
          sync: arguments.length > 2 && !async,
        };
        return open.apply(this, arguments);
      };
      win.XMLHttpRequest.prototype.setRequestHeader = function (name, value) {
        if (this.__sikemuxCall) this.__sikemuxCall.headers[name] = value;
        return setRequestHeader.apply(this, arguments);
      };
      win.XMLHttpRequest.prototype.abort = function () {
        this.__sikemuxCall = null;
        return abort.apply(this, arguments);
      };
      win.XMLHttpRequest.prototype.send = function () {
        const call = this.__sikemuxCall;
        if (call && held(call.method, call.url)) throw new TypeError(HELD);
        const waiting =
          call && !call.sync && waitForPerson(call.method, call.url);
        if (!waiting) return recordedSend.apply(this, arguments);
        const args = arguments;
        waiting.then((go) => {
          if (this.__sikemuxCall !== call) return;
          if (go) {
            try {
              recordedSend.apply(this, args);
              return;
            } catch {}
          }
          Object.defineProperty(this, "readyState", {
            value: 4,
            configurable: true,
          });
          this.dispatchEvent(new Event("readystatechange"));
          for (const type of ["error", "loadend"])
            this.dispatchEvent(new ProgressEvent(type));
        });
      };
      const recordedSend = function (body) {
        const call = this.__sikemuxCall;
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
    }

    if (win.Navigator && typeof win.Navigator.prototype.sendBeacon === "function") {
      const sendBeacon = win.Navigator.prototype.sendBeacon;
      win.Navigator.prototype.sendBeacon = function (url) {
        if (held("POST", url)) throw new TypeError(HELD);
        return sendBeacon.apply(this, arguments);
      };
    }

    if (typeof win.WebSocket === "function") {
      const NativeWebSocket = win.WebSocket;
      const WrappedWebSocket = function (url, protocols) {
        if (held("WEBSOCKET", url)) throw new TypeError(HELD);
        return arguments.length > 1
          ? new NativeWebSocket(url, protocols)
          : new NativeWebSocket(url);
      };
      WrappedWebSocket.prototype = NativeWebSocket.prototype;
      Object.setPrototypeOf(WrappedWebSocket, NativeWebSocket);
      win.WebSocket = WrappedWebSocket;
    }

    if (win.HTMLFormElement && win.HTMLFormElement.prototype) {
      const submit = win.HTMLFormElement.prototype.submit;
      win.HTMLFormElement.prototype.submit = function () {
        if (formHeld(this, null)) throw new TypeError(HELD);
        return submit.apply(this, arguments);
      };
      const requestSubmit = win.HTMLFormElement.prototype.requestSubmit;
      if (typeof requestSubmit === "function") {
        win.HTMLFormElement.prototype.requestSubmit = function (submitter) {
          if (formHeld(this, submitter || null)) throw new TypeError(HELD);
          return requestSubmit.apply(this, arguments);
        };
      }
    }
    win.addEventListener(
      "submit",
      (event) => {
        if (formHeld(event.target, event.submitter || null)) {
          event.preventDefault();
          event.stopImmediatePropagation();
        }
      },
      true,
    );

    if (typeof win.setTimeout === "function") {
      const nativeSetTimeout = win.setTimeout;
      win.setTimeout = function (fn) {
        if (watch || agentDepth > 0 || fromAgent(1)) {
          const args = Array.prototype.slice.call(arguments);
          args[0] = wrapTainted(fn, win);
          return nativeSetTimeout.apply(this, args);
        }
        return nativeSetTimeout.apply(this, arguments);
      };
    }
    if (typeof win.setInterval === "function") {
      const nativeSetInterval = win.setInterval;
      win.setInterval = function (fn) {
        if (watch || agentDepth > 0 || fromAgent(1)) {
          const args = Array.prototype.slice.call(arguments);
          args[0] = wrapTainted(fn, win);
          return nativeSetInterval.apply(this, args);
        }
        return nativeSetInterval.apply(this, arguments);
      };
    }
    if (typeof win.requestAnimationFrame === "function") {
      const nativeRaf = win.requestAnimationFrame;
      win.requestAnimationFrame = function (fn) {
        if (watch || agentDepth > 0 || fromAgent(1)) {
          return nativeRaf.call(this, wrapTainted(fn, win));
        }
        return nativeRaf.apply(this, arguments);
      };
    }
    if (typeof win.queueMicrotask === "function") {
      const nativeQueue = win.queueMicrotask;
      win.queueMicrotask = function (fn) {
        if (watch || agentDepth > 0 || fromAgent(1)) {
          return nativeQueue.call(this, wrapTainted(fn, win));
        }
        return nativeQueue.apply(this, arguments);
      };
    }

    if (win.HTMLIFrameElement && win.HTMLIFrameElement.prototype) {
      const winDesc = Object.getOwnPropertyDescriptor(
        win.HTMLIFrameElement.prototype,
        "contentWindow",
      );
      if (winDesc && winDesc.get) {
        Object.defineProperty(win.HTMLIFrameElement.prototype, "contentWindow", {
          configurable: true,
          get: function () {
            const child = winDesc.get.call(this);
            try {
              if (child && child.document) installIn(child);
            } catch {}
            return child;
          },
        });
      }
      const docDesc = Object.getOwnPropertyDescriptor(
        win.HTMLIFrameElement.prototype,
        "contentDocument",
      );
      if (docDesc && docDesc.get) {
        Object.defineProperty(
          win.HTMLIFrameElement.prototype,
          "contentDocument",
          {
            configurable: true,
            get: function () {
              const doc = docDesc.get.call(this);
              try {
                if (doc && doc.defaultView) installIn(doc.defaultView);
              } catch {}
              return doc;
            },
          },
        );
      }
    }
  };

  installIn(window);

  Object.defineProperty(window, "__sikemuxNet", {
    value: Object.freeze({
      entries: () => entries,
      watch: (token, allowed) => {
        if (!checkToken(token)) return;
        const list = Array.isArray(allowed) ? allowed.slice() : [];
        lastAllowed = list;
        watch = { allowed: list, write: null };
      },
      release: (token) => {
        if (!checkToken(token)) return null;
        const write = watch && watch.write;
        watch = null;
        return write;
      },
      hold: (token) => {
        if (!checkToken(token)) return;
        const refuse = () =>
          answer(
            stopHolding().map((write) => write.id),
            false,
          );
        refuse();
        holding = [];
        holdEnds = setTimeout(refuse, HOLD_MS);
      },
      held: (token) => (checkToken(token) ? stopHolding() : []),
      answer: (token, ids, go) => {
        if (!checkToken(token)) return;
        answer(ids, go);
      },
    }),
    writable: false,
    configurable: false,
  });
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
