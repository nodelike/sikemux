import { useRef, useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import { router } from 'expo-router';
import { CameraView, useCameraPermissions } from 'expo-camera';
import { parsePairingLink } from '@sikemux/native';

import { openFoundLink, pasteFoundLink } from '@/devices/foundLinks';
import { Icon } from '@/ui/Icon';
import { Button, useBottomGap } from '@/ui/parts';
import { fonts } from '@/ui/theme';

const FINDER = 236;

export default function Scan() {
  const [permission, requestPermission] = useCameraPermissions();
  const handled = useRef(false);
  const [found, setFound] = useState(false);

  const bottom = useBottomGap();
  return (
    <View style={styles.screen}>
      {permission?.granted ? (
        <CameraView
          style={StyleSheet.absoluteFill}
          barcodeScannerSettings={{ barcodeTypes: ['qr'] }}
          onBarcodeScanned={
            found
              ? undefined
              : ({ data }) => {
                  const link = parsePairingLink(data);
                  if (!link || handled.current) return;
                  handled.current = true;
                  setFound(true);
                  openFoundLink(link);
                }
          }
        />
      ) : null}
      <View style={styles.shade} pointerEvents="none">
        <View style={styles.finder}>
          <View style={[styles.corner, styles.tl]} />
          <View style={[styles.corner, styles.tr]} />
          <View style={[styles.corner, styles.bl]} />
          <View style={[styles.corner, styles.br]} />
        </View>
        <Text style={styles.hint}>
          {permission && !permission.granted && !permission.canAskAgain
            ? 'Allow the camera for Sikemux in Settings, or paste the pairing link.'
            : 'On your Mac, open Settings → Devices and point the camera at the code.'}
        </Text>
      </View>
      <SafeAreaView edges={['top']} style={styles.top}>
        <Pressable style={styles.back} onPress={() => router.back()} accessibilityRole="button" accessibilityLabel="Back">
          <View style={{ transform: [{ rotate: '180deg' }] }}>
            <Icon name="IconChevron" size={18} color="#fff" />
          </View>
          <Text style={styles.backText}>Back</Text>
        </Pressable>
      </SafeAreaView>
      <View style={[styles.bottom, { bottom }]}>
        {permission && !permission.granted && permission.canAskAgain ? (
          <Button kind="primary" title="Allow the camera" onPress={requestPermission} style={{ marginBottom: 8 }} />
        ) : null}
        <Button title="Paste a pairing link instead" onPress={() => pasteFoundLink('replace')} style={styles.glass} />
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: '#101013' },
  shade: { ...StyleSheet.absoluteFill, alignItems: 'center', paddingTop: '62%' },
  finder: { width: FINDER, height: FINDER, marginTop: -FINDER / 2 },
  corner: { position: 'absolute', width: 44, height: 44, borderColor: '#fff' },
  tl: { left: 0, top: 0, borderLeftWidth: 3, borderTopWidth: 3, borderTopLeftRadius: 26 },
  tr: { right: 0, top: 0, borderRightWidth: 3, borderTopWidth: 3, borderTopRightRadius: 26 },
  bl: { left: 0, bottom: 0, borderLeftWidth: 3, borderBottomWidth: 3, borderBottomLeftRadius: 26 },
  br: { right: 0, bottom: 0, borderRightWidth: 3, borderBottomWidth: 3, borderBottomRightRadius: 26 },
  hint: { marginTop: 32, marginHorizontal: 28, textAlign: 'center', color: '#fff', fontFamily: fonts.ui, fontSize: 15, lineHeight: 22 },
  top: { position: 'absolute', left: 0, right: 0, top: 0 },
  back: { flexDirection: 'row', alignItems: 'center', height: 46, paddingHorizontal: 10 },
  backText: { color: '#fff', fontFamily: fonts.ui, fontSize: 16, marginLeft: 2 },
  bottom: { position: 'absolute', left: 16, right: 16 },
  glass: { backgroundColor: 'rgba(28, 28, 34, 0.82)', borderColor: 'rgba(255, 255, 255, 0.12)' },
});
