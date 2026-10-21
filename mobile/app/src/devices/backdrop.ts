import { useEffect, useState } from 'react';
import { File, Paths } from 'expo-file-system';

import { connectionOf, reloadDevices, useDevices, useLive } from './hub';
import { updateDevice, type PairedDevice } from './paired';
import { failures, hostFolder, pathOf } from './savedFiles';

export type DeviceBackdrop = { texture: boolean; image?: string };

/** A host hands over whichever picture it has now, so each host has one fetch at a time. */
const fetching = new Set<string>();
/** Pictures the host said it does not have; asking again on every change of its view would not help. */
const missing = new Set<string>();
/** When fetching each picture last failed, to wait before asking again. */
const failed = failures();
/** The picture each host's view names now, to tell a fetch that finished after its host moved on. */
const latest = new Map<string, string>();

function folder(core: string) {
  return hostFolder(Paths.document, 'backdrops', core);
}

/* iOS moves an app's documents on every update, so a saved picture is looked for by its name in the
   host's folder first, and by the path it was saved at only when it is not there. */
function savedPicture(core: string, image: { uri: string }): File {
  const here = new File(folder(core), image.uri.slice(image.uri.lastIndexOf('/') + 1));
  return here.exists ? here : new File(image.uri);
}

/** Deletes the picture saved for a host's backdrop. */
export function forgetBackdrop(core: string, saved: PairedDevice['backdrop']) {
  if (!saved?.image) return;
  const file = savedPicture(core, saved.image);
  if (file.exists) file.delete();
}

/** What a host draws behind its panes: as it publishes it, or as last seen. Its picture is fetched only when it changes. */
export function useDeviceBackdrop(core: string): DeviceBackdrop {
  const live = useLive(core);
  const { devices } = useDevices();
  const remembered = devices.find((device) => device.core === core)?.backdrop;
  const published = live.snapshot?.workspace.backdrop;
  const texture = published?.texture;
  const wanted = published ? (published.image ?? null) : undefined;
  const connection = connectionOf(live);

  const [fetches, setFetches] = useState(0);

  useEffect(() => {
    if (texture === undefined || wanted === undefined) return;
    if (wanted && wanted !== remembered?.image?.id) {
      const key = `${core}/${wanted}`;
      latest.set(core, wanted);
      if (!connection || fetching.has(core) || missing.has(key)) return;
      if (failed.recently(key)) return;
      fetching.add(core);
      // The Rust client decodes and writes the picture, so megabytes of it never pass through JavaScript.
      connection
        .saveBackdrop(pathOf(folder(core).uri), wanted)
        .then((path) => {
          if (!path) {
            missing.add(key);
            return;
          }
          const saved = new File(`file://${path}`);
          // Saved under the name asked for, but it is whatever the host shows now, which may already be another.
          if (latest.get(core) !== wanted) {
            if (saved.exists) saved.delete();
            return;
          }
          failed.clear(key);
          forgetBackdrop(core, remembered);
          return updateDevice(core, { backdrop: { texture, image: { id: wanted, uri: saved.uri } } }).then(reloadDevices);
        })
        .catch(() => failed.record(key))
        .finally(() => {
          fetching.delete(core);
          setFetches((count) => count + 1);
        });
      return;
    }
    if (!wanted && remembered?.image) forgetBackdrop(core, remembered);
    if (texture !== remembered?.texture || (!wanted && remembered?.image)) {
      updateDevice(core, { backdrop: { texture, image: wanted ? remembered?.image : undefined } }).then(reloadDevices, () => {});
    }
  }, [core, connection, texture, wanted, remembered, fetches]);

  const shown = texture ?? remembered?.texture ?? false;
  const current = wanted === undefined ? remembered?.image?.id : wanted;
  const image = current && remembered?.image?.id === current ? savedPicture(core, remembered.image).uri : undefined;
  return { texture: shown, image };
}
