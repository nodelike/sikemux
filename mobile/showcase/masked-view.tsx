/* The fade under the pinned tabs can't be drawn in a browser, so the showcase shows the content without it. */
import type { ReactNode } from 'react';
import { View, type StyleProp, type ViewStyle } from 'react-native';

export default function MaskedView({ style, children }: { style?: StyleProp<ViewStyle>; children?: ReactNode; maskElement?: ReactNode }) {
  return <View style={style}>{children}</View>;
}
