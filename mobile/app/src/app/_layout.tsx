import { useEffect } from 'react';
import { Stack } from 'expo-router';
import * as SplashScreen from 'expo-splash-screen';
import { StatusBar } from 'expo-status-bar';
import { useFonts } from 'expo-font';
import { Figtree_400Regular, Figtree_400Regular_Italic, Figtree_500Medium, Figtree_600SemiBold } from '@expo-google-fonts/figtree';
import { JetBrainsMono_400Regular } from '@expo-google-fonts/jetbrains-mono';

import { useColors } from '@/ui/theme';

SplashScreen.preventAutoHideAsync();

export default function RootLayout() {
  const colors = useColors();
  const [loaded, failed] = useFonts({
    Figtree_400Regular,
    Figtree_400Regular_Italic,
    Figtree_500Medium,
    Figtree_600SemiBold,
    JetBrainsMono_400Regular,
  });

  useEffect(() => {
    if (loaded || failed) SplashScreen.hideAsync();
  }, [loaded, failed]);

  if (!loaded && !failed) return null;
  return (
    <>
      <StatusBar style="light" />
      <Stack screenOptions={{ headerShown: false, contentStyle: { backgroundColor: colors.ground } }} />
    </>
  );
}
