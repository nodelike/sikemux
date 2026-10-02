import { useEffect } from 'react';
import { Directory, File, Paths } from 'expo-file-system';

import { reloadDevices, useDevices, useLive } from './hub';
import { updateDevice, type PairedDevice } from './paired';

export type DeviceBackdrop = { texture: boolean; image?: string };

const fetching = new Set<string>();
/** Pictures the Mac could not hand over; asking again on every change of its view would not help. */
const missing = new Set<string>();

function folder(core: string): Directory {
  return new Directory(Paths.document, 'backdrops', core.slice(0, 16));
}

/** Deletes the picture saved for a Mac's backdrop. */
export function forgetBackdrop(saved: PairedDevice['backdrop']) {
  if (!saved?.image) return;
  const file = new File(saved.image.uri);
  if (file.exists) file.delete();
}

/** What a Mac draws behind its panes: as it publishes it, or as last seen. Its picture is fetched only when it changes. */
export function useDeviceBackdrop(core: string): DeviceBackdrop {
  const live = useLive(core);
  const { devices } = useDevices();
  const remembered = devices.find((device) => device.core === core)?.backdrop;
  const published = live.snapshot?.workspace.backdrop;
  const texture = published?.texture;
  const wanted = published ? (published.image ?? null) : undefined;
  const connection = live.status === 'open' ? live.connection : undefined;

  useEffect(() => {
    if (texture === undefined || wanted === undefined) return;
    if (wanted && wanted !== remembered?.image?.id) {
      const key = `${core}/${wanted}`;
      if (!connection || fetching.has(core) || missing.has(key)) return;
      fetching.add(core);
      // The Rust client decodes and writes the picture, so megabytes of it never pass through JavaScript.
      const dir = decodeURIComponent(folder(core).uri.replace(/^file:\/\//, ''));
      connection
        .saveBackdrop(dir, wanted)
        .then((path) => {
          if (!path) {
            missing.add(key);
            return;
          }
          forgetBackdrop(remembered);
          return updateDevice(core, { backdrop: { texture, image: { id: wanted, uri: new File(`file://${path}`).uri } } }).then(
            reloadDevices,
          );
        })
        .catch(() => missing.add(key))
        .finally(() => fetching.delete(core));
      return;
    }
    if (!wanted && remembered?.image) forgetBackdrop(remembered);
    if (texture !== remembered?.texture || (!wanted && remembered?.image)) {
      updateDevice(core, { backdrop: { texture, image: wanted ? remembered?.image : undefined } }).then(reloadDevices, () => {});
    }
  }, [core, connection, texture, wanted, remembered]);

  const shown = texture ?? remembered?.texture ?? false;
  const current = wanted === undefined ? remembered?.image?.id : wanted;
  const image = current && remembered?.image?.id === current ? remembered.image.uri : undefined;
  return { texture: shown, image };
}
