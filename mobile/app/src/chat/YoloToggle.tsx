import { useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import MaskedView from '@react-native-masked-view/masked-view';
import Svg, { Defs, LinearGradient, Rect, Stop } from 'react-native-svg';

import { canSwitchMode } from '@shared/agents/catalog';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { fonts, isLight, type Palette, useColors, useStyles } from '@/ui/theme';

const rainbow = (colors: Palette) => [colors.accent, colors.cmd, colors.danger, colors.warn, colors.live, colors.accent];

/** YOLO's rainbow along the composer's edge, as the Mac's composer draws it while the agent runs without asking. */
export function YoloEdge({ radius }: { radius: number }) {
  const colors = useColors();
  const [size, setSize] = useState({ width: 0, height: 0 });
  const stops = rainbow(colors);
  return (
    <View
      style={StyleSheet.absoluteFill}
      pointerEvents="none"
      onLayout={(event) => setSize({ width: event.nativeEvent.layout.width, height: event.nativeEvent.layout.height })}>
      {size.width ? (
        <Svg width={size.width} height={size.height} opacity={isLight(colors) ? 0.6 : 0.45}>
          <Defs>
            <LinearGradient id="yolo-edge" x1="0" y1="0" x2="1" y2="0">
              {stops.map((color, index) => (
                <Stop key={index} offset={index / (stops.length - 1)} stopColor={color} />
              ))}
            </LinearGradient>
          </Defs>
          <Rect
            x={0.5}
            y={0.5}
            width={size.width - 1}
            height={size.height - 1}
            rx={radius - 0.5}
            fill="none"
            stroke="url(#yolo-edge)"
            strokeWidth={1}
          />
        </Svg>
      ) : null}
    </View>
  );
}

function YoloWord() {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const stops = rainbow(colors);
  return (
    <MaskedView maskElement={<Text style={styles.yoloText}>yolo</Text>}>
      <Text style={[styles.yoloText, { opacity: 0 }]} importantForAccessibility="no" accessibilityElementsHidden>
        yolo
      </Text>
      <Svg style={StyleSheet.absoluteFill}>
        <Defs>
          {/* Twice the word's width, as the Mac draws it, so the word shows the first half. */}
          <LinearGradient id="yolo" x1="0" y1="0" x2="2" y2="0">
            {stops.map((color, index) => (
              <Stop key={index} offset={index / (stops.length - 1)} stopColor={color} />
            ))}
          </LinearGradient>
        </Defs>
        <Rect x="0" y="0" width="100%" height="100%" fill="url(#yolo)" />
      </Svg>
    </MaskedView>
  );
}

/** YOLO or safe, as the Mac's toggle says it; a tap switches it where the host can. */
export function YoloToggle({
  core,
  mode,
  provider,
  locked,
  onToggle,
}: {
  /** The host the agent runs on. */
  core: string;
  mode: string;
  provider: string;
  /** A turn is running or something waits on an answer, and the host refuses a switch then. */
  locked: boolean;
  onToggle: (mode: string) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const on = mode === 'bypass';
  const switches = canSwitchMode(provider, core);
  const look = (
    <>
      <Icon name={on ? 'IconShieldBolt' : 'IconShield'} size={13} color={on ? colors.accent : colors.inkFaint} />
      {on ? <YoloWord /> : <Text style={styles.yoloText}>safe</Text>}
    </>
  );
  if (!switches) {
    return (
      <View style={styles.yolo} accessible accessibilityLabel={on ? 'Runs without asking' : 'Asks before acting'}>
        {look}
      </View>
    );
  }
  return (
    <Pressable
      onPress={() => {
        haptics.select();
        onToggle(on ? 'workspace-write' : 'bypass');
      }}
      disabled={locked}
      style={({ pressed }) => [styles.yolo, pressed && { backgroundColor: colors.active }, locked && { opacity: 0.55 }]}
      accessibilityRole="switch"
      accessibilityLabel="Run without asking"
      accessibilityState={{ checked: on, disabled: locked }}
      accessibilityHint={locked ? 'Changes once the turn ends' : undefined}>
      {look}
    </Pressable>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    yolo: { flexDirection: 'row', alignItems: 'center', gap: 4, height: 34, paddingHorizontal: 8, borderRadius: 7 },
    yoloText: { fontFamily: fonts.uiSemibold, fontSize: 11, letterSpacing: 0.9, textTransform: 'uppercase', color: colors.inkFaint },
  });
};
