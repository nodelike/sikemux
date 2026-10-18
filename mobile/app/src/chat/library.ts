import { useEffect, useRef, useState } from 'react';
import {
  addListener,
  AssetField,
  MediaType,
  presentPermissionsPicker,
  Query,
  requestPermissionsAsync,
  type AssetMetadata,
  type PermissionResponse,
} from 'expo-media-library';

import { useAppActive } from '@/ui/motion';

export type LibraryPhoto = Pick<AssetMetadata, 'id' | 'filename' | 'width' | 'height'>;

/** `asking` until the person answers; `limited` when they gave the app only some photos. */
export type LibraryAccess = 'asking' | 'all' | 'limited' | 'denied';

/** A multiple of the grid's four columns, so a page never leaves a row half full. */
const PAGE = 96;

function accessOf(permission: PermissionResponse): LibraryAccess {
  if (!permission.granted) return 'denied';
  return permission.accessPrivileges === 'limited' ? 'limited' : 'all';
}

async function newestPhotos(offset: number): Promise<LibraryPhoto[]> {
  const found = await new Query()
    .eq(AssetField.MEDIA_TYPE, MediaType.IMAGE)
    .orderBy({ key: AssetField.CREATION_TIME, ascending: false })
    .limit(PAGE)
    .offset(offset)
    .exeForMetadata();
  return found.map(({ id, filename, width, height }) => ({ id, filename, width, height }));
}

/**
 * The newest photos on the phone, a page at a time, while `open`. Access is asked for the first time
 * it opens, never before, and checked again when the app comes back from Settings.
 */
export function useLibrary(open: boolean) {
  const active = useAppActive();
  const [access, setAccess] = useState<LibraryAccess>('asking');
  // The phone shows its prompt again only while this holds, which iOS allows once.
  const [canAsk, setCanAsk] = useState(true);
  const [photos, setPhotos] = useState<LibraryPhoto[]>([]);
  const [ended, setEnded] = useState(false);
  // Bumped by every fresh load, so a page that arrives after one is dropped.
  const load = useRef({ generation: 0, busy: false });

  const reload = () => {
    const generation = ++load.current.generation;
    load.current.busy = true;
    newestPhotos(0)
      .then((page) => {
        if (generation !== load.current.generation) return;
        setPhotos(page);
        setEnded(page.length < PAGE);
      })
      .catch(() => setEnded(true))
      .finally(() => {
        if (generation === load.current.generation) load.current.busy = false;
      });
  };

  const ask = () =>
    requestPermissionsAsync(false, ['photo']).then((permission) => {
      setAccess(accessOf(permission));
      setCanAsk(permission.canAskAgain);
    });

  useEffect(() => {
    if (open && active) void ask();
    // ask only sets state.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, active]);

  const sees = access === 'all' || access === 'limited';
  useEffect(() => {
    if (!open || !sees) return;
    reload();
    const watching = addListener(reload);
    return () => {
      watching.remove();
      load.current.generation += 1;
      load.current.busy = false;
    };
    // reload only reads refs and state setters.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, sees]);

  const [wasOpen, setWasOpen] = useState(open);
  if (wasOpen !== open) {
    setWasOpen(open);
    if (!open && photos.length > PAGE) setPhotos(photos.slice(0, PAGE));
  }

  const more = () => {
    if (ended || load.current.busy || !photos.length) return;
    const generation = load.current.generation;
    load.current.busy = true;
    newestPhotos(photos.length)
      .then((page) => {
        if (generation !== load.current.generation) return;
        setPhotos((now) => [...now, ...page]);
        setEnded(page.length < PAGE);
      })
      .catch(() => setEnded(true))
      .finally(() => {
        if (generation === load.current.generation) load.current.busy = false;
      });
  };

  return { access, canAsk, photos, more, ask: () => void ask(), manage: () => void presentPermissionsPicker(['photo']) };
}
