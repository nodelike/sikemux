// Tauri's dialog plugin swaps alert and confirm for its own asynchronous
// versions in every webview, tabs included, where a site would get back a
// promise instead of an answer. A fresh frame still holds the browser's own.
(() => {
    let natives = null;
    const native = (name) => {
        if (!natives) {
            const frame = document.createElement("iframe");
            frame.style.display = "none";
            (document.body || document.documentElement).appendChild(frame);
            natives = { alert: frame.contentWindow.alert, confirm: frame.contentWindow.confirm };
            frame.remove();
        }
        return natives[name];
    };
    window.alert = function alert() {
        return native("alert").apply(window, arguments);
    };
    window.confirm = function confirm() {
        return native("confirm").apply(window, arguments);
    };
})();
