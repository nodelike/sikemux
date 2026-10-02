import { useEffect, useMemo } from 'react';

import { paletteFrom, type Palette } from '@/ui/theme';
import { reloadDevices, useDevices, useLive } from './hub';
import { updateDevice } from './paired';

const known = new Map<string, Palette>();

/** One palette object per set of colours, so styles built for it are built once. */
function stable(colours: Record<string, string> | undefined): Palette {
  const key = JSON.stringify(colours ?? {});
  let palette = known.get(key);
  if (!palette) {
    palette = paletteFrom(colours);
    known.set(key, palette);
  }
  return palette;
}

/** The theme of the Mac a device screen belongs to: as it publishes it, or as last seen. */
export function useDevicePalette(core: string): Palette {
  const live = useLive(core);
  const { devices } = useDevices();
  const remembered = devices.find((device) => device.core === core)?.palette;
  const colours = live.snapshot?.workspace.palette;
  const published = useMemo(() => (colours?.size ? Object.fromEntries(colours) : undefined), [colours]);
  const current = published ?? remembered;

  useEffect(() => {
    if (!published) return;
    if (JSON.stringify(published) === JSON.stringify(remembered)) return;
    updateDevice(core, { palette: published }).then(reloadDevices, () => {});
  }, [core, published, remembered]);

  return useMemo(() => stable(current), [current]);
}
