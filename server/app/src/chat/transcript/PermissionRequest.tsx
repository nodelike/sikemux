import { IconShieldBolt } from "@shared/ui/Icons";
import type { PermissionAsk } from "@shared/wire.generated";

export function PermissionRequest({ request }: { request: PermissionAsk }) {
  return (
    <section
      className="chat-permission"
      aria-label={`Permission required for ${request.toolCall.title}`}
    >
      <div className="chat-permission-copy">
        <IconShieldBolt size={16} />
        <div>
          <strong>{request.toolCall.title}</strong>
          <span>Waiting for permission before this tool can continue.</span>
        </div>
      </div>
    </section>
  );
}
