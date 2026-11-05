import type { ReactNode } from 'react';
import { ScrollView, View, type StyleProp, type ViewStyle } from 'react-native';

/* While a view around a scroller holds the touch, iOS won't let the scroller pan, so the
   scroller's own content takes it before the message's press handler can. */
export const keepTouch = () => true;

/** Scrolls sideways inside the transcript, so long lines keep to the width of the screen. */
export function Sideways({
  children,
  contentStyle,
  style,
}: {
  children: ReactNode;
  contentStyle: StyleProp<ViewStyle>;
  style?: StyleProp<ViewStyle>;
}) {
  return (
    <ScrollView horizontal contentContainerStyle={contentStyle} showsHorizontalScrollIndicator={false}>
      <View onStartShouldSetResponder={keepTouch} style={style}>
        {children}
      </View>
    </ScrollView>
  );
}
