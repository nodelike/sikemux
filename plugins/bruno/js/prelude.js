// What a Bruno script is written against: bru, req, res, expect, test and console, over the
// run's variables, request and response. The run comes in as the JSON text __input; what the
// script left behind goes back as the JSON text __output. __send(json) is the host's own send,
// which applies the person's trust again to every request a script makes.
(function () {
  const input = JSON.parse(__input);
  const { Assertion, expect } = __chai;

  Assertion.addMethod("status", function (expected) {
    const actual = this._obj;
    const status = actual && typeof actual === "object" && "status" in actual ? actual.status : actual;
    this.assert(status === expected, "expected status #{act} to be #{exp}", "expected status #{act} not to be #{exp}", expected, status);
  });

  const fmt = (value) => {
    if (typeof value === "string") return value;
    try {
      return JSON.stringify(value);
    } catch {
      return String(value);
    }
  };
  const errMsg = (error) =>
    error && typeof error === "object" && typeof error.message === "string" ? error.message : String(error);

  const includes = (actual, v) => {
    if (typeof actual === "string") return actual.includes(String(v));
    if (Array.isArray(actual)) return actual.includes(v);
    if (actual && typeof actual === "object") return String(v) in actual;
    return false;
  };
  const isEmpty = (v) => {
    if (v == null) return true;
    if (typeof v === "string" || Array.isArray(v)) return v.length === 0;
    if (typeof v === "object") return Object.keys(v).length === 0;
    return false;
  };
  const coerce = (s) => {
    const t = s.trim();
    if (t === "") return "";
    try {
      return JSON.parse(t);
    } catch {
      return t.replace(/^['"]|['"]$/g, "");
    }
  };
  // An assert row's right side, `<op> <expected>`, held against what its expression gave.
  const compare = (actual, op, expectedRaw) => {
    const expected = coerce(expectedRaw);
    const num = Number(actual);
    const fail = (error) => ({ passed: false, error });
    const pass = { passed: true };
    switch (op) {
      case "eq":
        return actual == expected ? pass : fail(`expected ${fmt(actual)} == ${fmt(expected)}`);
      case "neq":
        return actual != expected ? pass : fail(`expected ${fmt(actual)} != ${fmt(expected)}`);
      case "gt":
        return num > Number(expected) ? pass : fail(`expected ${fmt(actual)} > ${fmt(expected)}`);
      case "gte":
        return num >= Number(expected) ? pass : fail(`expected ${fmt(actual)} >= ${fmt(expected)}`);
      case "lt":
        return num < Number(expected) ? pass : fail(`expected ${fmt(actual)} < ${fmt(expected)}`);
      case "lte":
        return num <= Number(expected) ? pass : fail(`expected ${fmt(actual)} <= ${fmt(expected)}`);
      case "contains":
        return includes(actual, expected) ? pass : fail(`expected ${fmt(actual)} to contain ${fmt(expected)}`);
      case "notContains":
        return !includes(actual, expected) ? pass : fail(`expected ${fmt(actual)} not to contain ${fmt(expected)}`);
      case "in":
        return Array.isArray(expected) && expected.includes(actual) ? pass : fail(`expected ${fmt(actual)} in ${fmt(expected)}`);
      case "notIn":
        return Array.isArray(expected) && !expected.includes(actual) ? pass : fail(`expected ${fmt(actual)} not in ${fmt(expected)}`);
      case "length":
        return actual?.length === Number(expected) ? pass : fail(`expected length ${fmt(expected)}`);
      case "matches":
        return new RegExp(String(expected)).test(String(actual)) ? pass : fail(`expected ${fmt(actual)} to match ${fmt(expected)}`);
      case "notMatches":
        return !new RegExp(String(expected)).test(String(actual)) ? pass : fail(`expected ${fmt(actual)} not to match ${fmt(expected)}`);
      case "startsWith":
        return String(actual).startsWith(String(expected)) ? pass : fail(`expected ${fmt(actual)} to start with ${fmt(expected)}`);
      case "endsWith":
        return String(actual).endsWith(String(expected)) ? pass : fail(`expected ${fmt(actual)} to end with ${fmt(expected)}`);
      case "isEmpty":
        return isEmpty(actual) ? pass : fail(`expected ${fmt(actual)} to be empty`);
      case "notEmpty":
        return !isEmpty(actual) ? pass : fail(`expected ${fmt(actual)} not to be empty`);
      case "isNull":
        return actual === null ? pass : fail("expected null");
      case "isUndefined":
        return actual === undefined ? pass : fail("expected undefined");
      case "isDefined":
        return actual !== undefined ? pass : fail("expected defined");
      case "isTruthy":
        return actual ? pass : fail("expected truthy");
      case "isFalsy":
        return !actual ? pass : fail("expected falsy");
      case "isNumber":
        return typeof actual === "number" ? pass : fail("expected number");
      case "isString":
        return typeof actual === "string" ? pass : fail("expected string");
      case "isBoolean":
        return typeof actual === "boolean" ? pass : fail("expected boolean");
      case "isArray":
        return Array.isArray(actual) ? pass : fail("expected array");
      case "between": {
        const parts = expectedRaw.trim().split(/\s+/).map(Number);
        return num >= parts[0] && num <= parts[1] ? pass : fail(`expected ${fmt(actual)} between ${parts[0]} and ${parts[1]}`);
      }
      default:
        return actual == expected ? pass : fail(`expected ${fmt(actual)} == ${fmt(expected)}`);
    }
  };

  const finish = (output) => {
    globalThis.__output = JSON.stringify(output);
  };

  if (input.expression !== undefined) {
    let value;
    try {
      const evaluate = new Function("res", "req", "bru", "return (" + input.expression + ");");
      value = evaluate(input.res, input.req, input.bru);
    } catch (error) {
      finish({ error: errMsg(error) });
      return;
    }
    if (input.op !== undefined) finish(compare(value, input.op, input.expected));
    else finish(value === undefined ? { undefined: true } : { value });
    return;
  }

  const ctx = { vars: input.vars, req: input.req, res: input.res, logs: [], tests: [] };
  const get = (key) => ctx.vars[key];
  const set = (key, value) => {
    ctx.vars[key] = value == null ? "" : String(value);
  };
  const bru = {
    getEnvVar: get,
    setEnvVar: set,
    getVar: get,
    setVar: set,
    getCollectionVar: get,
    getProcessEnv: (key) => ctx.vars["process.env." + key] || "",
    setNextRequest: () => {},
    sendRequest: async (opts) => {
      const headers = Object.entries(opts.headers || {}).map(([key, value]) => [key, String(value)]);
      const data = opts.data;
      const body =
        data == null
          ? { kind: "none" }
          : typeof data === "string"
            ? { kind: "raw", content_type: null, data }
            : { kind: "raw", content_type: "application/json", data: JSON.stringify(data) };
      const answer = JSON.parse(__send(JSON.stringify({ method: (opts.method || "GET").toUpperCase(), url: opts.url, headers, body })));
      if (answer.error) throw new Error(answer.error);
      const response = answer.response;
      let parsed = response.body;
      try {
        parsed = JSON.parse(response.body);
      } catch {
        // Not JSON, so the script gets the text.
      }
      return { status: response.status, statusText: response.status_text, headers: Object.fromEntries(response.headers), data: parsed };
    },
  };
  const r = ctx.req;
  const req = {
    getUrl: () => r.url,
    setUrl: (url) => {
      r.url = url;
    },
    getMethod: () => r.method,
    setMethod: (method) => {
      r.method = method;
    },
    getHeaders: () => r.headers,
    getHeader: (key) => r.headers[key],
    setHeader: (key, value) => {
      r.headers[key] = value;
    },
    getBody: () => r.body,
    setBody: (body) => {
      r.body = body;
    },
    getName: () => r.name,
  };
  const s = ctx.res;
  const res = s && {
    get status() {
      return s.status;
    },
    get body() {
      return s.body;
    },
    get headers() {
      return s.headers;
    },
    get responseTime() {
      return s.responseTime;
    },
    getStatus: () => s.status,
    getBody: () => s.body,
    getHeader: (key) => s.headers[key],
    getHeaders: () => s.headers,
    getResponseTime: () => s.responseTime,
  };
  const pending = [];
  const test = (name, fn) => {
    try {
      const result = fn();
      if (result && typeof result.then === "function") {
        pending.push(
          result.then(
            () => ctx.tests.push({ name, passed: true }),
            (error) => ctx.tests.push({ name, passed: false, error: errMsg(error) }),
          ),
        );
      } else {
        ctx.tests.push({ name, passed: true });
      }
    } catch (error) {
      ctx.tests.push({ name, passed: false, error: errMsg(error) });
    }
  };
  const log =
    (level) =>
    (...args) =>
      ctx.logs.push({ level, text: args.map(fmt).join(" ") });
  const console = { log: log("log"), info: log("info"), warn: log("warn"), error: log("error"), debug: log("debug") };
  const done = () => finish({ vars: ctx.vars, req: ctx.req, logs: ctx.logs, tests: ctx.tests });
  let script;
  try {
    script = new Function("bru", "req", "res", "expect", "test", "console", "return (async () => {\n" + input.code + "\n})();");
  } catch (error) {
    ctx.logs.push({ level: "error", text: "script error: " + errMsg(error) });
    done();
    return;
  }
  Promise.resolve()
    .then(() => script(bru, req, res, expect, test, console))
    .then(() => Promise.all(pending))
    .catch((error) => ctx.logs.push({ level: "error", text: "script error: " + errMsg(error) }))
    .then(done);
})();
