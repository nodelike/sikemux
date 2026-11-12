import { useEffect } from 'react';
import { StyleSheet, View } from 'react-native';

import { useT } from '@/i18n';
import { Button } from '@/ui/controls';
import { PromptScreen } from '@/ui/prompt';
import { useColors } from '@/ui/theme';
import { unlock, useLock } from './lock';

/** What shows until the person passes Face ID, Touch ID or the passcode. It asks by itself as it opens. */
export function LockScreen() {
  const { asking } = useLock();
  const t = useT();
  useEffect(() => {
    void unlock();
  }, []);

  return (
    <PromptScreen
      icon="IconLock"
      title={t('Sikemux is locked')}
      actions={<Button kind="primary" title={t('Unlock')} busy={asking} onPress={() => void unlock()} />}
    />
  );
}

/**
 * Covers the app while it is away, so the app switcher's picture shows nothing, and the lock once it has been
 * away a while. A modal shows over the screens, so each one puts its own cover last inside it.
 */
export function LockCover() {
  const { phase } = useLock();
  const colors = useColors();
  if (phase === 'open') return null;
  return <View style={[StyleSheet.absoluteFill, { backgroundColor: colors.ground }]}>{phase === 'covered' ? null : <LockScreen />}</View>;
}
