import { memo } from 'react';
import { StyleSheet, View } from 'react-native';

import type { RemoteAgent } from '@/core/protocol';
import { AgentIcon } from '@/ui/Icon';
import { Row } from '@/ui/list';
import { AgentState, Dot } from '@/ui/status';
import { useColors } from '@/ui/theme';

function WorkerEnd({ worker }: { worker: RemoteAgent }) {
  const colors = useColors();
  if (!worker.reachable) return <Dot color={colors.rest} hollow />;
  return <AgentState group={worker.group} />;
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
