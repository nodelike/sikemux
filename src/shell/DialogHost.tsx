import { useCallback, useEffect, useRef, useState } from "react";
import { acceptDialog, chooseOther, dismissDialog, useDialogs, type PendingDialog } from "../state/dialog";
import { IconInfo, IconWarning } from "../ui/Icons";
import { Modal } from "../ui/Modal";

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
    const [value, setValue] = useState(dialog.kind === "prompt" ? (dialog.initial ?? "") : "");
    const inputRef = useRef<HTMLInputElement>(null);
    const confirmRef = useRef<HTMLButtonElement>(null);
    const cancelRef = useRef<HTMLButtonElement>(null);

    const accept = () => acceptDialog(dialog.id, value);
    const dismiss = useCallback(() => dismissDialog(dialog.id), [dialog.id]);
    // Destructive confirms focus cancel so a stray Return never deletes anything.
    const destructive = dialog.kind !== "prompt" && !!dialog.destructive;

    useEffect(() => {
        if (dialog.kind === "prompt") {
            inputRef.current?.focus();
            inputRef.current?.select();
        } else (destructive ? cancelRef : confirmRef).current?.focus();
    }, [dialog.kind, destructive]);

    const paragraphs = (dialog.body ?? "").split("\n").filter((line) => line.trim().length > 0);

    return (
        <Modal
            title={dialog.title}
            glyph={destructive ? <IconWarning size={15} /> : <IconInfo size={15} />}
            danger={destructive}
            onClose={dismiss}
            foot={
                <>
                    <button ref={cancelRef} type="button" className="dlg-btn" onClick={dismiss}>
                        {dialog.cancelLabel ?? "Cancel"}
                    </button>
                    {dialog.kind === "choice" && (
                        <button type="button" className="dlg-btn" onClick={() => chooseOther(dialog.id)}>
                            {dialog.otherLabel}
                        </button>
                    )}
                    <button
                        ref={confirmRef}
                        type="button"
                        className={`dlg-btn primary${destructive ? " danger" : ""}`}
                        disabled={dialog.kind === "prompt" && value.trim().length === 0}
                        onClick={accept}>
                        {dialog.confirmLabel ?? (dialog.kind === "prompt" ? "Save" : "Confirm")}
                    </button>
                </>
            }>
            {paragraphs.length > 0 && (
                <div className="dlg-body">
                    {paragraphs.map((line, i) => (
                        <p key={i}>{line}</p>
                    ))}
                </div>
            )}
            {dialog.kind !== "prompt" && dialog.commands && dialog.commands.length > 0 && (
                <ul className="dlg-commands">
                    {dialog.commands.map((entry, i) => (
                        <li key={i}>
                            <span className="dlg-command-label">{entry.label}</span>
                            <code className="dlg-command">{entry.command}</code>
                        </li>
                    ))}
                </ul>
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
        </Modal>
    );
}
