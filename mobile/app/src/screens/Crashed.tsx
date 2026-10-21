import { StyleSheet, Text } from 'react-native';
import type { ErrorBoundaryProps } from 'expo-router';

import { goHome } from '@/ui/navigate';
import { Button } from '@/ui/controls';
import { alertFailure, PromptScreen } from '@/ui/prompt';
import { type Palette, typeFor, useStyles } from '@/ui/theme';

/** What a screen shows in place of itself when drawing it threw. */
export function Crashed({ error, retry }: ErrorBoundaryProps) {
  const styles = useStyles(makeStyles);
  return (
    <PromptScreen
      icon="IconWarning"
      title="Something went wrong"
      actions={
        <>
          <Button kind="primary" title="Try again" onPress={() => retry().catch(alertFailure("Couldn't try again"))} />
          <Button kind="text" title="Go to Devices" onPress={goHome} />
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
