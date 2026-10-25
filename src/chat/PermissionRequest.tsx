import { IconShieldBolt } from "../ui/Icons";
import type { PermissionAsk } from "@shared/chat/types";

export function PermissionRequest({
    request,
    busy,
    onReply,
}: {
    request: PermissionAsk;
    busy: boolean;
    /** Without it, as in a shared chat, the request is shown waiting on someone else. */
    onReply?: (optionId?: string) => void;
}) {
    return (
        <section className="chat-permission" aria-label={`Permission required for ${request.toolCall.title}`}>
            <div className="chat-permission-copy">
                <IconShieldBolt size={16} />
                <div>
                    <strong>{request.toolCall.title}</strong>
                    <span>
                        {onReply ? "Agent needs permission before this tool can continue." : "Waiting for permission before this tool can continue."}
                    </span>
                </div>
            </div>
            {onReply && (
                <div className="chat-permission-actions">
                    {request.options.map((option) => (
                        <button
                            key={option.optionId}
                            type="button"
                            disabled={busy}
                            className={option.kind.startsWith("reject") ? "reject" : "allow"}
                            onClick={() => onReply(option.optionId)}>
                            {option.name}
                        </button>
                    ))}
                    {!request.options.some((option) => option.kind.startsWith("reject")) && (
                        <button type="button" disabled={busy} className="reject" onClick={() => onReply()}>
                            Cancel
                        </button>
                    )}
                </div>
            )}
        </section>
    );
}
