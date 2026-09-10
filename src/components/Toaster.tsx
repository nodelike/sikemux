import { useToasts } from "../state/toast";
import { IconClose } from "./Icons";

export function Toaster() {
    const toasts = useToasts((s) => s.toasts);
    const dismiss = useToasts((s) => s.dismiss);
    const pause = useToasts((s) => s.pause);
    const resume = useToasts((s) => s.resume);
    if (toasts.length === 0) return null;
    return (
        <div className="toaster" aria-live="polite" aria-atomic="false">
            {toasts.map((t) => (
                <div
                    key={t.id}
                    onMouseEnter={() => pause(t.id, "pointer")}
                    onMouseLeave={() => resume(t.id, "pointer")}
                    onFocusCapture={() => pause(t.id, "focus")}
                    onBlurCapture={(event) => {
                        if (!event.currentTarget.contains(event.relatedTarget)) resume(t.id, "focus");
                    }}
                    className={`toast toast-${t.kind}`}
                    role={t.kind === "error" ? "alert" : "status"}>
                    <span className="toast-text">{t.text}</span>
                    {t.action && (
                        <button
                            className="toast-action"
                            onClick={() => {
                                if (t.action?.dismissOnClick) dismiss(t.id);
                                void t.action?.run(t.id);
                            }}>
                            {t.action.label}
                        </button>
                    )}
                    <button className="toast-x" onClick={() => dismiss(t.id)} title="Dismiss" aria-label="Dismiss notification">
                        <IconClose size={10} />
                    </button>
                </div>
            ))}
        </div>
    );
}
