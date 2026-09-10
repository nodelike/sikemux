import { useModalFocus } from "../hooks/useModalFocus";
import { useEffect, useRef, useState } from "react";
import { acceptDialog, dismissDialog, useDialogs, type PendingDialog } from "../state/dialog";
import { IconInfo, IconWarning } from "./Icons";
import { Kbd } from "./Kbd";

/**
 * Renders the app's own confirm/prompt sheet in place of the platform dialogs.
 * Mounted once at the app root; `state/dialog` owns the queue.
 */
export function DialogHost() {
    const dialog = useDialogs((s) => s.dialog);
    if (!dialog) return null;
    // Key on id so the prompt input resets between queued dialogs.
    return <DialogSheet key={dialog.id} dialog={dialog} />;
}

function DialogSheet({ dialog }: { dialog: PendingDialog }) {
    const modalRef = useRef<HTMLDivElement>(null);
    useModalFocus(modalRef);
    const [value, setValue] = useState(dialog.kind === "prompt" ? (dialog.initial ?? "") : "");
    const inputRef = useRef<HTMLInputElement>(null);
    const confirmRef = useRef<HTMLButtonElement>(null);
    const cancelRef = useRef<HTMLButtonElement>(null);

    const accept = () => acceptDialog(dialog.id, value);
    const dismiss = () => dismissDialog(dialog.id);
    // Destructive confirms focus cancel so a stray Return never deletes anything.
    const destructive = dialog.kind === "confirm" && !!dialog.destructive;

    useEffect(() => {
        if (dialog.kind === "prompt") {
            inputRef.current?.focus();
            inputRef.current?.select();
        } else (destructive ? cancelRef : confirmRef).current?.focus();
    }, [dialog.kind, destructive]);

    // Capture Escape before the panes behind the scrim can act on it.
    useEffect(() => {
        const onKey = (event: KeyboardEvent) => {
            if (event.key !== "Escape" || (event.target instanceof Element && event.target.closest(".dd-menu"))) return;
            event.preventDefault();
            event.stopPropagation();
            dismiss();
        };
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    });

    const paragraphs = (dialog.body ?? "").split("\n").filter((line) => line.trim().length > 0);

    return (
        <div className="dlg-scrim" onMouseDown={dismiss}>
            <div
                ref={modalRef}
                tabIndex={-1}
                className={`dlg${destructive ? " danger" : ""}`}
                role="dialog"
                aria-modal="true"
                aria-label={dialog.title}
                onMouseDown={(event) => event.stopPropagation()}>
                <div className="dlg-head">
                    <span className="dlg-glyph" aria-hidden="true">
                        {destructive ? <IconWarning size={15} /> : <IconInfo size={15} />}
                    </span>
                    <h2 className="dlg-title">{dialog.title}</h2>
                </div>
                {paragraphs.length > 0 && (
                    <div className="dlg-body">
                        {paragraphs.map((line, i) => (
                            <p key={i}>{line}</p>
                        ))}
                    </div>
                )}
                {dialog.kind === "prompt" && (
                    <form
                        className="dlg-field"
                        onSubmit={(event) => {
                            event.preventDefault();
                            accept();
                        }}>
                        {dialog.label && <label htmlFor={`dlg-input-${dialog.id}`}>{dialog.label}</label>}
                        <input
                            id={`dlg-input-${dialog.id}`}
                            ref={inputRef}
                            className="dlg-input"
                            value={value}
                            placeholder={dialog.placeholder}
                            spellCheck={false}
                            autoComplete="off"
                            onChange={(event) => setValue(event.target.value)}
                        />
                    </form>
                )}
                <div className="dlg-foot">
                    <span className="dlg-hint">
                        <Kbd>esc</Kbd> cancel
                    </span>
                    <button ref={cancelRef} type="button" className="dlg-btn" onClick={dismiss}>
                        {dialog.cancelLabel ?? "Cancel"}
                    </button>
                    <button
                        ref={confirmRef}
                        type="button"
                        className={`dlg-btn primary${destructive ? " danger" : ""}`}
                        disabled={dialog.kind === "prompt" && value.trim().length === 0}
                        onClick={accept}>
                        {dialog.confirmLabel ?? (dialog.kind === "prompt" ? "Save" : "Confirm")}
                    </button>
                </div>
            </div>
        </div>
    );
}
