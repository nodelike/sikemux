import { useMemo } from 'react';

import { useLive } from '@/devices/hub';
import type { ConfigPicker, RecentModel } from '@shared/wire.generated';

/** The name of the value a picker holds, or the value itself when its choices do not list it. */
export function currentLabel(picker: ConfigPicker | null | undefined): string | undefined {
  if (!picker) return undefined;
  return picker.choices.find((choice) => choice.value === picker.current)?.label ?? (picker.current || undefined);
}

/** The agents the host can start, each once, in the order it lists them. */
export function useAgents(core: string): string[] {
  const launchers = useLive(core).snapshot?.workspace.launchers;
  return useMemo(() => [...new Set((launchers ?? []).map((launcher) => launcher.provider))], [launchers]);
}

/** The models new chats of `provider` started on lately on the host, newest first. */
export function useRecentModels(core: string, provider: string): RecentModel[] | undefined {
  return useLive(core).snapshot?.workspace.launchers.find((launcher) => launcher.provider === provider)?.recentModels;
}
