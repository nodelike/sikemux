import { useEffect, useState } from 'react';
import * as SecureStore from 'expo-secure-store';

import { CHIEF_ROLE } from '@shared/calls/words';
import type { Snapshot } from '@/core/protocol';
import { knownDevices, liveOf, onAnyHost, useDevices, watch } from '@/devices/hub';
import { deviceName, type PairedDevice } from '@/devices/paired';
import { translate } from '@/i18n';

/** A project's chief the share sheet offers to put what is shared in, as an unsent draft attachment. */
export type ShareChief = {
  host: string;
  hostName: string;
  projectId: string;
  project: string;
  chatId: string;
  provider: string;
  activeAt: number;
};

type OpenProject = { core: string; project: string };

const LAST_PROJECT_KEY = 'sikemux.last-open-project';
let lastOpen: OpenProject | undefined;
let loaded: Promise<void> | undefined;

function loadLastOpen() {
  loaded ??= SecureStore.getItemAsync(LAST_PROJECT_KEY)
    .then((stored) => {
      if (!stored) return;
      const parsed = JSON.parse(stored) as Partial<OpenProject>;
      if (typeof parsed.core === 'string' && typeof parsed.project === 'string') lastOpen ??= { core: parsed.core, project: parsed.project };
    })
    .catch(() => {});
  return loaded;
}

/** Remembers the project the phone had open last, so its chief leads the share sheet. */
export function noteOpenProject(core: string, project: string | undefined) {
  if (!project) return;
  if (lastOpen?.core === core && lastOpen.project === project) return;
  lastOpen = { core, project };
  SecureStore.setItemAsync(LAST_PROJECT_KEY, JSON.stringify(lastOpen)).catch(() => {});
}

function lastOpenOf(hosts: readonly { device: PairedDevice; snapshot: Snapshot | undefined }[]): OpenProject | undefined {
  if (lastOpen) return lastOpen;
  let scoped: PairedDevice | undefined;
  for (const { device } of hosts) {
    if (!device.project) continue;
    if (!scoped || (device.lastSeen ?? 0) > (scoped.lastSeen ?? 0)) scoped = device;
  }
  return scoped?.project ? { core: scoped.core, project: scoped.project } : undefined;
}

/**
 * One row per project that has a chief across every host this phone has full access to, ordered with
 * the project the phone last had open first, then most recently active.
 */
export function shareChiefs(
  hosts: readonly { device: PairedDevice; snapshot: Snapshot | undefined }[],
  last: OpenProject | undefined = lastOpenOf(hosts),
): ShareChief[] {
  const found: (ShareChief & { order: number })[] = [];
  let order = 0;
  for (const { device, snapshot } of hosts) {
    if (device.access !== 'full' || !snapshot) continue;
    const hostName = deviceName(device, translate);
    const seenProjects = new Set<string>();
    for (const project of snapshot.workspace.projects) {
      if (!project.chief || project.chiefHome) continue;
      seenProjects.add(project.id);
      const chatId = project.chief;
      const chat = snapshot.chats.find((item) => item.agentId === chatId);
      const shown =
        snapshot.inbox.agents.find((agent) => agent.agentId === chatId) ??
        snapshot.inbox.kept?.find((agent) => agent.agentId === chatId);
      const recent = snapshot.recent.find((item) => item.agentId === chatId || (item.project === project.id && item.title === CHIEF_ROLE));
      const provider = chat?.provider ?? shown?.cli ?? project.launcher?.split(':')[0] ?? 'agent';
      found.push({
        host: device.core,
        hostName,
        projectId: project.id,
        project: project.name,
        chatId,
        provider,
        activeAt: shown?.activeAt ?? chat?.activeAt ?? recent?.activeAt ?? 0,
        order: order++,
      });
    }
    // Older cores without project.chief still mark open chiefs in inbox.agents.
    for (const shown of snapshot.inbox.agents) {
      if (!shown.chief || !shown.project || seenProjects.has(shown.project)) continue;
      seenProjects.add(shown.project);
      const project = snapshot.workspace.projects.find((item) => item.id === shown.project);
      found.push({
        host: device.core,
        hostName,
        projectId: shown.project,
        project: project?.name ?? shown.projectName ?? shown.project,
        chatId: shown.agentId,
        provider: shown.cli,
        activeAt: shown.activeAt ?? 0,
        order: order++,
      });
    }
  }
  const isLast = (chief: ShareChief) => last != null && chief.host === last.core && chief.projectId === last.project;
  found.sort((a, b) => Number(isLast(b)) - Number(isLast(a)) || b.activeAt - a.activeAt || a.order - b.order);
  return found.map(({ order: _, ...chief }) => chief);
}

function shareChiefsNow(): ShareChief[] {
  return shareChiefs(knownDevices().map((device) => ({ device, snapshot: liveOf(device.core).snapshot })));
}

/** The share sheet's project chiefs, kept current while it shows; every host with full access stays connected meanwhile. */
export function useShareChiefs(): ShareChief[] {
  const { devices } = useDevices();
  useEffect(() => {
    const stops = devices.filter((device) => device.access === 'full').map((device) => watch(device.core));
    return () => stops.forEach((stop) => stop());
  }, [devices]);
  const [chiefs, setChiefs] = useState(shareChiefsNow);
  useEffect(() => {
    let active = true;
    const refresh = () => {
      const next = shareChiefsNow();
      setChiefs((shown) => (JSON.stringify(shown) === JSON.stringify(next) ? shown : next));
    };
    void loadLastOpen().then(() => {
      if (active) refresh();
    });
    const stop = onAnyHost(refresh);
    return () => {
      active = false;
      stop();
    };
  }, []);
  return chiefs;
}
