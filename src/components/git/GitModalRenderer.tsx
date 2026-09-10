import { useModalFocus } from "../../hooks/useModalFocus";
import { useEffect, useRef, useState } from "react";
import { closeGitModal, dispatchGitMenuKey } from "../../state/git";
import { getState, useStore } from "../../state/store";
import { PRIMARY_SHORTCUT } from "../../lib/platform";

type ConfirmModal = Extract<NonNullable<ReturnType<typeof useStore.getState>["gitModal"]>, { kind: "confirm" }>;

function submitConfirmation(modal: ConfirmModal): void {
    if (getState().gitModal !== modal) return;
    closeGitModal();
    void modal.onConfirm();
}

export function GitModalRenderer({ paneId, active }: { paneId: string; active: boolean }) {
    const modal = useStore((s) => s.gitModal);
    const ownsModal = !!modal && modal.ownerPaneId === paneId;
    const modalRef = useRef<HTMLDivElement>(null);
    useModalFocus(modalRef, ownsModal && active);

    useEffect(() => {
        return () => {
            if (getState().gitModal?.ownerPaneId === paneId) closeGitModal();
        };
    }, [paneId]);

    useEffect(() => {
        if (!modal || !ownsModal) return;
        if (!active) {
            closeGitModal();
            return;
        }
        const onKey = (e: KeyboardEvent) => {
            if (e.key === "Escape") {
                e.preventDefault();
                e.stopPropagation();
                closeGitModal();
                return;
            }
            if (modal.kind === "confirm" && modal.confirmKey === e.key && !e.repeat && !e.metaKey && !e.ctrlKey && !e.altKey) {
                e.preventDefault();
                e.stopPropagation();
                submitConfirmation(modal);
                return;
            }
            if (modal.kind === "menu" && dispatchGitMenuKey(e.key)) {
                e.preventDefault();
                e.stopPropagation();
            }
        };
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    }, [modal, ownsModal, active]);

    if (!modal || !ownsModal || !active) return null;
    return (
        <div className="dlg-scrim" onClick={closeGitModal}>
            <div
                ref={modalRef}
                tabIndex={-1}
                className={`dlg git-modal git-modal-${modal.kind}`}
                role="dialog"
                aria-modal="true"
                aria-label={modal.title}
                onClick={(e) => e.stopPropagation()}>
                {modal.kind === "menu" && <MenuBody modal={modal} />}
                {modal.kind === "confirm" && <ConfirmBody modal={modal} />}
                {modal.kind === "prompt" && <PromptBody modal={modal} />}
                {modal.kind === "cheatsheet" && <CheatsheetBody modal={modal} />}
            </div>
        </div>
    );
}

function MenuBody({ modal }: { modal: Extract<NonNullable<ReturnType<typeof useStore.getState>["gitModal"]>, { kind: "menu" }> }) {
    return (
        <>
            <div className="dlg-head">
                <h2 className="dlg-title">{modal.title}</h2>
            </div>
            <div className="dlg-body git-modal-body">
                {modal.items.map((item, i) => (
                    <button
                        key={i}
                        type="button"
                        disabled={item.disabled}
                        className={`git-menu-item${item.destructive ? " danger" : ""}`}
                        onClick={() => {
                            closeGitModal();
                            void item.run();
                        }}>
                        {item.key && <span className="git-menu-key">{item.key}</span>}
                        <span className="git-menu-label">{item.label}</span>
                        {item.hint && <span className="git-menu-hint">{item.hint}</span>}
                    </button>
                ))}
            </div>
            <div className="dlg-foot">
                <span className="kbd">esc</span> cancel
            </div>
        </>
    );
}

function ConfirmBody({ modal }: { modal: ConfirmModal }) {
    const confirmRef = useRef<HTMLButtonElement>(null);
    const cancelRef = useRef<HTMLButtonElement>(null);
    useEffect(() => {
        const target = modal.initialFocus ?? (modal.destructive ? "cancel" : "confirm");
        (target === "confirm" ? confirmRef : cancelRef).current?.focus();
    }, [modal.destructive, modal.initialFocus]);
    return (
        <>
            <div className="dlg-head">
                <h2 className="dlg-title">{modal.title}</h2>
            </div>
            <div className="dlg-body git-modal-confirm">{modal.body}</div>
            <div className="dlg-foot">
                <button ref={cancelRef} type="button" className="dlg-btn" onClick={closeGitModal}>
                    {modal.cancelLabel ?? "cancel"}
                </button>
                <button
                    ref={confirmRef}
                    type="button"
                    className={`dlg-btn primary${modal.destructive ? " danger" : ""}`}
                    onClick={() => submitConfirmation(modal)}>
                    {modal.confirmLabel ?? "confirm"}
                </button>
            </div>
        </>
    );
}

function PromptBody({ modal }: { modal: Extract<NonNullable<ReturnType<typeof useStore.getState>["gitModal"]>, { kind: "prompt" }> }) {
    const [value, setValue] = useState(modal.initial ?? "");
    const inputRef = useRef<HTMLInputElement | HTMLTextAreaElement>(null);
    useEffect(() => {
        inputRef.current?.focus();
        if (modal.initial && inputRef.current) inputRef.current.select();
    }, [modal.initial]);

    const submit = () => {
        closeGitModal();
        void modal.onConfirm(value);
    };

    const matching = (modal.suggestions ?? []).filter((s) => s.value.toLowerCase().includes(value.toLowerCase()));

    return (
        <>
            <div className="dlg-head">
                <h2 className="dlg-title">{modal.title}</h2>
            </div>
            <div className="dlg-body git-modal-prompt">
                {modal.multiline ? (
                    <textarea
                        ref={inputRef as React.RefObject<HTMLTextAreaElement>}
                        className="dlg-input git-modal-multiline"
                        placeholder={modal.placeholder}
                        value={value}
                        onChange={(e) => setValue(e.target.value)}
                        onKeyDown={(e) => {
                            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                                e.preventDefault();
                                submit();
                            }
                        }}
                        spellCheck={false}
                        autoCapitalize="off"
                        autoCorrect="off"
                    />
                ) : (
                    <input
                        ref={inputRef as React.RefObject<HTMLInputElement>}
                        type="text"
                        className="dlg-input"
                        placeholder={modal.placeholder}
                        value={value}
                        onChange={(e) => setValue(e.target.value)}
                        onKeyDown={(e) => {
                            if (e.key === "Enter") {
                                e.preventDefault();
                                submit();
                            }
                        }}
                        spellCheck={false}
                        autoCapitalize="off"
                        autoCorrect="off"
                    />
                )}
                {modal.suggestions && matching.length > 0 && (
                    <div className="git-modal-suggestions">
                        {matching.slice(0, 8).map((s) => (
                            <button
                                key={s.value}
                                type="button"
                                className="git-modal-suggestion"
                                onClick={() => {
                                    setValue(s.value);
                                    inputRef.current?.focus();
                                }}>
                                <span>{s.value}</span>
                                {s.hint && <span className="git-modal-suggestion-hint">{s.hint}</span>}
                            </button>
                        ))}
                    </div>
                )}
            </div>
            <div className="dlg-foot">
                <button type="button" className="dlg-btn" onClick={closeGitModal}>
                    cancel
                </button>
                <button type="button" className="dlg-btn primary" onClick={submit}>
                    {modal.multiline ? `submit (${PRIMARY_SHORTCUT}↵)` : "ok (↵)"}
                </button>
            </div>
        </>
    );
}

function CheatsheetBody({ modal }: { modal: Extract<NonNullable<ReturnType<typeof useStore.getState>["gitModal"]>, { kind: "cheatsheet" }> }) {
    return (
        <>
            <div className="dlg-head">
                <h2 className="dlg-title">{modal.title}</h2>
            </div>
            <div className="dlg-body git-modal-cheatsheet">
                {modal.sections.map((sec, si) => (
                    <div className="git-cheat-section" key={si}>
                        <div className="git-cheat-section-h">{sec.title}</div>
                        <div className="git-cheat-rows">
                            {sec.rows.map((row, ri) => (
                                <div className="git-cheat-row" key={ri}>
                                    <span className="git-cheat-keys">{row.keys}</span>
                                    <span className="git-cheat-label">{row.label}</span>
                                </div>
                            ))}
                        </div>
                    </div>
                ))}
            </div>
            <div className="dlg-foot">
                <span className="kbd">esc</span> close
            </div>
        </>
    );
}
