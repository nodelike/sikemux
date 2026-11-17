import type { TurnState } from '@shared/wire.generated';
import { AgentFace } from '@/devices/Buddy';
import { Icon } from '@/ui/Icon';
import { useColors } from '@/ui/theme';

/** An agent's mark where a call or a row names it: the project's chief wears Sikemux's S in the accent, any other agent its own face. */
export function AskerMark({
  agentId,
  provider,
  chief,
  turn,
  asking,
  size = 20,
}: {
  agentId: string;
  provider: string;
  chief?: boolean;
  turn?: TurnState;
  asking?: boolean;
  size?: number;
}) {
  const colors = useColors();
  if (chief) return <Icon name="Logo" size={size * 0.75} color={colors.accent} />;
  return <AgentFace agentId={agentId} provider={provider} turn={turn} asking={asking} size={size} />;
}
