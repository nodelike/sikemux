import { useEffect, useState } from 'react';

import type { Snapshot } from '@/core/protocol';
import { knownDevices, liveOf, onAnyHost, useDevices, watch } from '@/devices/hub';
import { deviceName, type PairedDevice } from '@/devices/paired';
import { projectAt, projectName, projectOf } from '@/devices/words';

/** A chat the share sheet offers to put what is shared in, as an unsent draft attachment. */
export type ShareChat = {
  host: string;
  hostName: string;
  chatId: string;
  provider: string;
  title: string;
  /** What the core calls the agent, such as Otter. */
  name?: string;
  project: string;
  activeAt: number;
};

/** The project a new chat would start in, on its host. */
type ShareNewChat = { host: string; hostName: string; project: string; projectPath: string };

export type ShareChats = { chats: ShareChat[]; newChat?: ShareNewChat };

const SHOWN = 4;

/** The chats used last across every host this phone has full access to, and a new chat in the newest one's project. */
function shareChats(hosts: readonly { device: PairedDevice; snapshot: Snapshot | undefined }[]): ShareChats {
  const found: (ShareChat & { projectPath: string })[] = [];
  for (const { device, snapshot } of hosts) {
    if (device.access !== 'full' || !snapshot) continue;
    const hostName = deviceName(device);
    for (const chat of snapshot.chats) {
      const shown = snapshot.inbox.agents.find((agent) => agent.agentId === chat.agentId);
      const where = projectOf(snapshot, chat.agentId) ?? chat.cwd;
      found.push({
        host: device.core,
        hostName,
        chatId: chat.agentId,
        provider: chat.provider,
        title: chat.title ?? shown?.title ?? 'New chat',
        name: chat.name ?? undefined,
        project: projectName(snapshot, where, chat.cwd),
        projectPath: projectAt(snapshot, where)?.path ?? chat.cwd,
        activeAt: shown?.activeAt ?? 0,
      });
    }
  }
  found.sort((a, b) => b.activeAt - a.activeAt);
  const first = found[0];
  return {
    chats: found.slice(0, SHOWN).map(({ projectPath: _, ...chat }) => chat),
    newChat: first && { host: first.host, hostName: first.hostName, project: first.project, projectPath: first.projectPath },
  };
}

/** The share sheet's chats from what the phone knows of its hosts now. */
function shareChatsNow(): ShareChats {
  return shareChats(knownDevices().map((device) => ({ device, snapshot: liveOf(device.core).snapshot })));
}

/** The share sheet's chats, kept current while it shows; every host with full access stays connected meanwhile. */
export function useShareChats(): ShareChats {
  const { devices } = useDevices();
  useEffect(() => {
    const stops = devices.filter((device) => device.access === 'full').map((device) => watch(device.core));
    return () => stops.forEach((stop) => stop());
  }, [devices]);
  const [chats, setChats] = useState(shareChatsNow);
  useEffect(
    () =>
      onAnyHost(() => {
        const next = shareChatsNow();
        setChats((shown) => (JSON.stringify(shown) === JSON.stringify(next) ? shown : next));
      }),
    [],
  );
  return chats;
}
