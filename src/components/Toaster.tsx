import { useEffect, useLayoutEffect, useRef } from "react";
import { animate, EASE_LEAVE, leavingRef } from "../lib/motion";
import { setNativeViewHoles, type NativeViewHole } from "../state/nativeViews";
import { useToasts, type ToastKind } from "../state/toast";
import { IconCheck, IconClose, IconExclamation, IconInfoMark } from "./Icons";

const KIND_ICON: Record<ToastKind, typeof IconCheck> = {
    success: IconCheck,
    error: IconExclamation,
    info: IconInfoMark,
};

/* A dismissed toast sinks and fades while its place, gap included, closes, so the ones beside it slide over. */
const dismissToast = leavingRef<HTMLDivElement>((toast) => {
    const gap = toast.parentElement ? parseFloat(getComputedStyle(toast.parentElement).rowGap) || 0 : 0;
    const before = toast.previousElementSibling ? -gap : 0;
    const after = !toast.previousElementSibling && toast.nextElementSibling ? -gap : 0;
    toast.style.overflow = "hidden";
    return animate(
        toast,
        [
            { opacity: 1, transform: "none", height: `${toast.offsetHeight}px`, minHeight: "0px" },
            {
                opacity: 0,
                transform: "translateY(6px) scale(0.98)",
                height: "0px",
                minHeight: "0px",
                paddingTop: "0px",
                paddingBottom: "0px",
                marginTop: `${before}px`,
                marginBottom: `${after}px`,
                borderWidth: "0px",
            },
        ],
        { duration: 120, easing: EASE_LEAVE },
    );
});

/* Where each toast rests, leaving aside its own entry and slide motion, so a
   browser page can leave a hole there instead of covering it. */
function restingRects(island: HTMLElement): NativeViewHole[] {
    const origin = island.getBoundingClientRect();
    return Array.from(island.querySelectorAll<HTMLElement>(":scope > .toast"), (toast) => ({
        x: Math.round(origin.left + toast.offsetLeft),
        y: Math.round(origin.top + toast.offsetTop),
        width: toast.offsetWidth,
        height: toast.offsetHeight,
        radius: parseFloat(getComputedStyle(toast).borderTopLeftRadius) || 0,
    })).filter((rect) => rect.width > 0 && rect.height > 0);
}

export function Toaster() {
    const toasts = useToasts((s) => s.toasts);
    const dismiss = useToasts((s) => s.dismiss);
    const pause = useToasts((s) => s.pause);
    const resume = useToasts((s) => s.resume);
    const island = useRef<HTMLDivElement>(null);
    const tops = useRef(new Map<string, number>());
    /* A new toast joins the bottom of the island, so the ones already up
       slide up to make room instead of jumping. */
    useLayoutEffect(() => {
        const next = new Map<string, number>();
        for (const el of island.current?.querySelectorAll<HTMLElement>(":scope > .toast[data-toast-id]:not(.is-leaving)") ?? []) {
            const id = el.dataset.toastId ?? "";
            const top = el.getBoundingClientRect().top;
            const was = tops.current.get(id);
            if (was !== undefined && Math.abs(was - top) > 0.5)
                animate(el, [{ transform: `translateY(${was - top}px)` }, { transform: "none" }], { duration: 200 });
            next.set(id, top);
        }
        tops.current = next;
    }, [toasts]);
    useEffect(() => {
        const node = island.current;
        if (!node) return;
        let frame = 0;
        const measure = () => {
            frame = 0;
            setNativeViewHoles(restingRects(node));
        };
        const schedule = () => {
            if (!frame) frame = requestAnimationFrame(measure);
        };
        measure();
        const observer = new ResizeObserver(schedule);
        observer.observe(node);
        window.addEventListener("resize", schedule);
        return () => {
            observer.disconnect();
            window.removeEventListener("resize", schedule);
            if (frame) cancelAnimationFrame(frame);
            setNativeViewHoles([]);
        };
    }, []);
    useLayoutEffect(() => setNativeViewHoles(island.current ? restingRects(island.current) : []), [toasts]);
    // The island stays mounted while empty: a live region has to exist before it is spoken into.
    return (
        <div ref={island} className="toaster" aria-live="polite" aria-atomic="false">
            {toasts.map((t) => {
                const KindIcon = KIND_ICON[t.kind];
                return (
                    <div
                        key={t.id}
                        ref={dismissToast}
                        data-toast-id={t.id}
                        onMouseEnter={() => pause(t.id, "pointer")}
                        onMouseLeave={() => resume(t.id, "pointer")}
                        onFocusCapture={() => pause(t.id, "focus")}
                        onBlurCapture={(event) => {
                            if (!event.currentTarget.contains(event.relatedTarget)) resume(t.id, "focus");
                        }}
                        className={`toast toast-${t.kind}`}
                        role={t.kind === "error" ? "alert" : "status"}>
                        <KindIcon size={14} className="toast-icon" />
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
                        {t.persistent && (
                            <button className="toast-x" onClick={() => dismiss(t.id)} title="Dismiss" aria-label="Dismiss notification">
                                <IconClose size={10} />
                            </button>
                        )}
                    </div>
                );
            })}
        </div>
    );
}
