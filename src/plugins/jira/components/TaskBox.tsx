import { createContext, useContext, useEffect, useState, type ChangeEvent } from "react";

/** Ticks or clears one task in the description; rejects when Jira refuses. */
export type ToggleTask = (index: number, text: string, done: boolean) => Promise<void>;

export const TaskToggleContext = createContext<ToggleTask | null>(null);

/** The task's words, without the words of any list nested under it. */
function taskText(box: HTMLInputElement): string {
    const item = box.closest("li");
    if (!item) return "";
    const copy = item.cloneNode(true) as HTMLElement;
    copy.querySelectorAll("ul, ol").forEach((nested) => nested.remove());
    return (copy.textContent ?? "").trim();
}

/** A task box in an issue's description that ticks the task in Jira. Its place among the description's boxes says which task it is. */
export function TaskBox({ checked }: { checked: boolean }) {
    const toggle = useContext(TaskToggleContext);
    const [shown, setShown] = useState<boolean | null>(null);
    const [busy, setBusy] = useState(false);
    useEffect(() => setShown(null), [checked]);

    const change = async (event: ChangeEvent<HTMLInputElement>) => {
        const box = event.currentTarget;
        const scope = box.closest(".jira-description");
        if (!toggle || !scope) return;
        const index = [...scope.querySelectorAll("input.jira-task")].indexOf(box);
        const done = box.checked;
        setShown(done);
        setBusy(true);
        try {
            await toggle(index, taskText(box), done);
        } catch {
            setShown(null);
        } finally {
            setBusy(false);
        }
    };

    return (
        <input
            type="checkbox"
            className="jira-task"
            checked={shown ?? checked}
            disabled={!toggle || busy}
            aria-label={(shown ?? checked) ? "Mark as not done" : "Mark as done"}
            onChange={(event) => void change(event)}
        />
    );
}
