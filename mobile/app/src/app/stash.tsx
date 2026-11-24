import { useCallback, useEffect, useRef, useState } from 'react';
import { router, useLocalSearchParams } from 'expo-router';
import { useAuth } from '@clerk/expo';

import { useShareChiefs } from '@/stash/chats';
import { leaveToSharer, takeIncoming } from '@/stash/incoming';
import { ShareSheet } from '@/stash/ShareSheet';
import { Screen } from '@/ui/screen';

/** The share menu's sheet, opened when another app shares to Sikemux. */
export default function StashShare() {
  const { url } = useLocalSearchParams<{ url?: string }>();
  const [shared] = useState(() => takeIncoming() ?? (url ? { text: url, files: [] } : null));
  const chiefs = useShareChiefs();
  const { getToken } = useAuth();
  const latestGetToken = useRef(getToken);
  useEffect(() => {
    latestGetToken.current = getToken;
  });
  const done = useCallback(() => {
    if (router.canGoBack()) router.back();
    else router.replace('/');
    leaveToSharer();
  }, []);
  if (!shared) return null;
  return (
    <Screen>
      <ShareSheet shared={shared} chiefs={chiefs} token={() => latestGetToken.current()} onDone={done} />
    </Screen>
  );
}
