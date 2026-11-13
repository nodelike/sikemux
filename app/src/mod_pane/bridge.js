(() => {
  const post = (message) =>
    window.webkit.messageHandlers.sikemux.postMessage(
      JSON.stringify({ kind: "mod", ...message }),
    );
  const waiting = new Map();
  const watching = new Map();
  const timers = new Set();
  const listeners = { shown: new Set(), hidden: new Set() };
  let next = 1;
  let shown = true;

  const ask = (op, name, params) =>
    new Promise((resolve, reject) => {
      const id = next++;
      waiting.set(id, { resolve, reject });
      post({ op, id, name, params: params ?? {} });
    });

  const tick = (timer) => {
    timer.last = Date.now();
    try {
      timer.run();
    } catch (error) {
      console.error(error);
    }
  };
  const arm = (timer) => {
    clearInterval(timer.handle);
    timer.handle = shown ? setInterval(() => tick(timer), timer.ms) : 0;
  };
  const every = (ms, run) => {
    const timer = { ms: Math.max(1000, Number(ms) || 0), run, last: 0, handle: 0 };
    timers.add(timer);
    if (shown) tick(timer);
    arm(timer);
    return () => {
      clearInterval(timer.handle);
      timers.delete(timer);
    };
  };

  window.sikemux = Object.freeze({
    read: (name, params) => ask("read", name, params),
    write: (name, params) => ask("write", name, params),
    watch(name, params, callback, options) {
      if (typeof params === "function") [params, callback, options] = [{}, params, callback];
      const id = next++;
      const watch = { callback, last: undefined, stop: null, every: options?.every ?? 60000 };
      watching.set(id, watch);
      post({ op: "watch", id, name, params: params ?? {} });
      return () => {
        watching.delete(id);
        watch.stop?.();
        post({ op: "unwatch", id });
      };
    },
    data: Object.freeze({
      get: (collection, id) => ask("read", "data.get", { collection, id }),
      list: (collection) => ask("read", "data.list", { collection }),
      set: (collection, id, doc) => ask("write", "data.set", { collection, id, doc }),
      patch: (collection, id, patch) => ask("write", "data.patch", { collection, id, patch }),
      remove: (collection, id) => ask("write", "data.remove", { collection, id }),
    }),
    badge: (text) => post({ op: "badge", text: text == null ? null : String(text) }),
    every,
    get shown() {
      return shown;
    },
    on(event, listener) {
      listeners[event]?.add(listener);
      return () => listeners[event]?.delete(listener);
    },
  });

  const deliver = (watch, error, value) => {
    const text = error ?? JSON.stringify(value);
    if (text === watch.last) return;
    watch.last = text;
    watch.callback(error == null ? value : undefined, error ?? undefined);
  };

  window.__sikemux = {
    answer(id, error, value) {
      const waiter = waiting.get(id);
      if (!waiter) return;
      waiting.delete(id);
      if (error == null) waiter.resolve(value);
      else waiter.reject(new Error(error));
    },
    watched(id, error, value) {
      const watch = watching.get(id);
      if (watch) deliver(watch, error, value);
    },
    timed(id, name, params) {
      const watch = watching.get(id);
      if (!watch) return;
      watch.stop = every(watch.every, () =>
        ask("read", name, params).then(
          (value) => deliver(watch, null, value),
          (error) => deliver(watch, error.message, undefined),
        ),
      );
    },
    shown(now) {
      if (shown === now) return;
      shown = now;
      for (const timer of timers) {
        if (now && Date.now() - timer.last >= timer.ms) tick(timer);
        arm(timer);
      }
      for (const listener of listeners[now ? "shown" : "hidden"]) listener();
    },
  };
})();
