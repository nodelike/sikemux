import { fireEvent } from "@testing-library/react";
import { draftOffset, draftPoint, readDraft } from "../chat/DraftEditor";

/** The composer's draft, as it would be sent. */
export function draftOf(editor: HTMLElement): string {
    return readDraft(editor);
}

function place(editor: HTMLElement, start: number, end: number): void {
    const range = document.createRange();
    const from = draftPoint(editor, start);
    const to = draftPoint(editor, end);
    range.setStart(from.node, from.offset);
    range.setEnd(to.node, to.offset);
    const selection = window.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(range);
}

export function selectDraft(editor: HTMLElement, start: number, end = start): void {
    place(editor, start, end);
    fireEvent(document, new Event("selectionchange"));
}

export function draftSelection(editor: HTMLElement): [number, number] {
    const range = window.getSelection()?.getRangeAt(0);
    if (!range) return [0, 0];
    return [draftOffset(editor, range.startContainer, range.startOffset), draftOffset(editor, range.endContainer, range.endOffset)];
}

/** Replaces the draft with `value` as if it were typed, leaving the caret at `caret`. */
export function typeDraft(editor: HTMLElement, value: string, caret = value.length): void {
    editor.focus();
    editor.textContent = value;
    place(editor, caret, caret);
    fireEvent.input(editor);
}
