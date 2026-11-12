import { memo } from 'react';
import { StyleSheet, View } from 'react-native';

import type { RemoteAgent } from '@shared/wire.generated';
import { AgentIcon } from '@/ui/Icon';
import { Row } from '@/ui/list';
import { AgentState, TurnMark } from '@/devices/AgentMarks';
import { useMinute } from '@/ui/motion';

/**
 * A worker's mark from its turn, or before its first turn from its group. One out of reach carries no mark, as on the
 * Mac's rail: the hollow ring is Stopped's alone.
 */
function WorkerEnd({ worker }: { worker: RemoteAgent }) {
  const minute = useMinute();
  if (!worker.reachable) return null;
  return worker.turn ? <TurnMark turn={worker.turn} now={minute * 60_000} /> : <AgentState group={worker.group} />;
}

/** A worker an agent here started on a linked host, set in when it sits under the agent that started it. */
export const WorkerRow = memo(function WorkerRow({ worker, nested }: { worker: RemoteAgent; nested?: boolean }) {
  return (
    <View style={nested && styles.nested}>
      <Row
        dim={!worker.reachable}
        mark={
          <View style={!worker.reachable && styles.away}>
            <AgentIcon provider={worker.cli} size={16} />
          </View>
        }
        name={worker.name}
        host={worker.host}
        place={worker.project}
        detail={worker.reachable ? worker.label : 'Out of reach'}
        end={<WorkerEnd worker={worker} />}
      />
    </View>
  );
});

export function workersOf(workers: readonly RemoteAgent[], lead: string): RemoteAgent[] {
  return workers.filter((worker) => worker.lead === lead);
}

const styles = StyleSheet.create({
  nested: { paddingLeft: 30 },
  away: { opacity: 0.5 },
});
