import { useEffect, useMemo } from 'react';

import { paletteFrom, type Palette } from '@/ui/theme';
import { reloadDevices, useDevices, useLive } from './hub';
import { updateDevice } from './paired';

/** The palettes used most recently, oldest first; enough for every paired host and a few theme changes. */
const known = new Map<string, Palette>();
const KEEP = 16;

/** One palette object per set of colours, so styles built for it are built once. */
function stable(colours: Record<string, string> | undefined): Palette {
  const key = JSON.stringify(colours ?? {});
  const palette = known.get(key) ?? paletteFrom(colours);
  known.delete(key);
  known.set(key, palette);
  if (known.size > KEEP) known.delete(known.keys().next().value!);
  return palette;
}

/** The theme of the host a device screen belongs to: as it publishes it, or as last seen. */
export function useDevicePalette(core: string): Palette {
  const live = useLive(core);
  const { devices } = useDevices();
  const remembered = devices.find((device) => device.core === core)?.palette;
  const colours = live.snapshot?.workspace.palette;
  const published = useMemo(() => (colours && Object.keys(colours).length ? colours : undefined), [colours]);
  const current = published ?? remembered;

  useEffect(() => {
    if (!published) return;
    if (JSON.stringify(published) === JSON.stringify(remembered)) return;
    updateDevice(core, { palette: published }).then(reloadDevices, () => {});
  }, [core, published, remembered]);

  return useMemo(() => stable(current), [current]);
}
