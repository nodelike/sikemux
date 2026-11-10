import { useState } from 'react';

import type { InboxItem, ShipHold, ShipKind } from '@shared/wire.generated';
import { Dock } from '@/chat/PermissionDock';

const TITLES: Record<ShipKind, string> = {
  push: 'Push commits?',
  merge: 'Merge a pull request?',
  deploy: 'Deploy?',
  publish: 'Publish?',
  productionWrite: 'Change production data?',
};

/** How the person answers a held action, as the host's calls name it. */
export type HoldAnswer = 'refuseShip' | 'allowShip' | 'letShipGo';

/**
 * An agent's actions that leave the host, held until the person answers: Refuse, Allow once and,
 * where the host offers it, Always allow what it lands in, which leads on a row of its own.
 */
export function HoldDock({
  items,
  answering,
  onAnswer,
}: {
  items: (InboxItem & { hold: ShipHold })[];
  answering: ReadonlySet<string>;
  onAnswer: (hold: ShipHold, how: HoldAnswer) => void;
}) {
  const [page, setPage] = useState(0);
  const index = Math.min(page, items.length - 1);
  const hold = items[index].hold;
  const title = TITLES[hold.kind] ?? 'Allow this?';
  const detail = hold.why ?? 'Sikemux held this until you allow it';
  return (
    <Dock
      title={title}
      detail={detail}
      label={`${title} ${detail}`}
      pager={{ index, count: items.length, onPage: setPage, noun: 'action' }}
      command={hold.what}
      run={false}
      busy={answering.has(hold.id)}
      acts={[
        ...(hold.letGo
          ? [
              {
                key: 'always',
                label: `Always allow ${hold.letGo}${hold.letGoIn ? ` in ${hold.letGoIn}` : ''}`,
                go: true,
                wide: true,
                onPress: () => onAnswer(hold, 'letShipGo'),
              },
            ]
          : []),
        { key: 'refuse', label: 'Refuse', quiet: true, onPress: () => onAnswer(hold, 'refuseShip') },
        { key: 'once', label: 'Allow once', go: !hold.letGo, onPress: () => onAnswer(hold, 'allowShip') },
      ]}
    />
  );
}
