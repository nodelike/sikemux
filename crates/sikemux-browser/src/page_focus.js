// Has the page answer hasFocus() and hear the window's focus and blur as if it had focus,
// while the person's real keyboard focus stays wherever it is.
((lend) => {
    let control = window.__sikemuxPageFocus;
    if (typeof control !== "function") {
        const own = Document.prototype.hasFocus;
        const really = () => own.call(document);
        let lent = false;
        Document.prototype.hasFocus = new Proxy(own, {
            apply: (target, self, args) => (lent && self === document) || Reflect.apply(target, self, args),
        });
        const held = (event) => {
            if (lent && event.isTrusted && event.target === window) event.stopImmediatePropagation();
        };
        window.addEventListener("focus", held, true);
        window.addEventListener("blur", held, true);
        control = (wanted) => {
            if (wanted === lent) return;
            lent = wanted;
            if (!really()) window.dispatchEvent(new FocusEvent(wanted ? "focus" : "blur"));
        };
        Object.defineProperty(window, "__sikemuxPageFocus", { value: control });
    }
    control(lend);
})
