import { useEffect } from 'react';
import { StyleSheet, Text } from 'react-native';
import type { ErrorBoundaryProps } from 'expo-router';

import { reportError } from '@/crashes/reports';
import { useT } from '@/i18n';
import { goHome } from '@/ui/navigate';
import { Button } from '@/ui/controls';
import { alertFailure, PromptScreen } from '@/ui/prompt';
import { type Palette, typeFor, useStyles } from '@/ui/theme';

/** What a screen shows in place of itself when drawing it threw. */
export function Crashed({ error, retry }: ErrorBoundaryProps) {
  const t = useT();
  const styles = useStyles(makeStyles);
  useEffect(() => reportError(error), [error]);
  return (
    <PromptScreen
      icon="IconWarning"
      title={t('Something went wrong')}
      actions={
        <>
          <Button kind="primary" title={t('Try again')} onPress={() => retry().catch(alertFailure(t("Couldn't try again")))} />
          <Button kind="text" title={t('Go to Devices')} onPress={goHome} />
        </>
      }>
      <Text style={styles.body} selectable>
        {error.message}
      </Text>
    </PromptScreen>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    body: { ...type.body, textAlign: 'center', marginTop: 8 },
  });
};
