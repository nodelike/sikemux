import { useCallback, useEffect, useRef, useState } from "react";
import { IconCheck, IconCopy } from "./Icons";
import { reportError } from "../state/toast";
import { copyText } from "../lib/clipboard";
import { Tooltip } from "./Tooltip";

const FEEDBACK_MS = 1100;

/** Small inline "copy this string" affordance — flips to a tick on success. */
export function CopyButton({
    value,
    label,
    className,
    size = 11,
}: {
    value: string;
    /** What the tooltip calls the copied thing, e.g. "branch name". */
    label: string;
    className?: string;
    size?: number;
}) {
    const [copied, setCopied] = useState(false);
    const timerRef = useRef<number | undefined>(undefined);

    useEffect(() => () => window.clearTimeout(timerRef.current), []);

    const copy = useCallback(() => {
        if (!value) return;
        copyText(value).then(
            () => {
                setCopied(true);
                window.clearTimeout(timerRef.current);
                timerRef.current = window.setTimeout(() => setCopied(false), FEEDBACK_MS);
            },
            reportError(`copy ${label}`),
        );
    }, [value, label]);

    return (
        <Tooltip label={copied ? `copied ${label}` : `Copy ${label}`}>
            <button
                type="button"
                className={`copy-btn${copied ? " copied" : ""}${className ? ` ${className}` : ""}`}
                disabled={!value}
                aria-label={`Copy ${label}`}
                onClick={(e) => {
                    e.stopPropagation();
                    copy();
                }}>
                {copied ? <IconCheck size={size} /> : <IconCopy size={size} />}
            </button>
        </Tooltip>
    );
}
