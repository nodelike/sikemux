import { useRef, useState } from "react";

/** Edits a chat's name in place. Enter or leaving the field saves, Escape cancels. */
export function AgentTitleInput({
    title,
    className,
    onSave,
    onDone,
}: {
    title: string;
    className: string;
    onSave: (title: string) => void;
    onDone: () => void;
}) {
    const [value, setValue] = useState(title);
    const finished = useRef(false);
    const finish = (save: boolean) => {
        if (finished.current) return;
        finished.current = true;
        if (save && value.trim() !== title) onSave(value);
        onDone();
    };
    return (
        <input
            className={`${className} agent-rename`}
            aria-label="Chat name"
            value={value}
            size={Math.max(value.length, 12)}
            maxLength={72}
            spellCheck={false}
            autoFocus
            onFocus={(event) => event.currentTarget.select()}
            onChange={(event) => setValue(event.target.value)}
            onBlur={() => finish(true)}
            onKeyDown={(event) => {
                if (event.key === "Enter") finish(true);
                else if (event.key === "Escape") finish(false);
                event.stopPropagation();
            }}
        />
    );
}
