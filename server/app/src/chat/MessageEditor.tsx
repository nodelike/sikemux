import { useEffect, useRef, useState } from "react";

/** A sent message opened again for writing. Sending it takes the chat back to
    before it and asks again with the new text. */
export function MessageEditor({
  initial,
  onSend,
  onCancel,
}: {
  initial: string;
  onSend: (text: string) => void;
  onCancel: () => void;
}) {
  const [text, setText] = useState(initial);
  const field = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    const element = field.current;
    if (!element) return;
    element.focus();
    element.setSelectionRange(element.value.length, element.value.length);
  }, []);

  useEffect(() => {
    const element = field.current;
    if (!element) return;
    element.style.height = "auto";
    element.style.height = `${element.scrollHeight}px`;
  }, [text]);

  const send = () => {
    if (text.trim()) onSend(text);
  };

  return (
    <div className="chat-message-editor">
      <textarea
        ref={field}
        value={text}
        rows={1}
        aria-label="Edit message"
        onChange={(event) => setText(event.target.value)}
        onKeyDown={(event) => {
          if (event.nativeEvent.isComposing) return;
          if (event.key === "Escape") {
            event.preventDefault();
            onCancel();
          } else if (
            event.key === "Enter" &&
            !event.shiftKey &&
            !event.altKey
          ) {
            event.preventDefault();
            send();
          }
        }}
      />
      <div className="chat-message-editor-actions">
        <button type="button" onClick={onCancel}>
          Cancel
        </button>
        <button
          type="button"
          className="primary"
          disabled={!text.trim()}
          onClick={send}
        >
          Send
        </button>
      </div>
    </div>
  );
}
