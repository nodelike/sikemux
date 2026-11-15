import { useMemo, type ReactNode } from 'react';
import { Keyboard, PanResponder, type StyleProp, View, type ViewStyle } from 'react-native';

const TAP = 8;

/** Ground that closes the keyboard when tapped or swiped down on, as a list does, for screens with nothing to scroll. */
export function KeyboardDismissArea({ style, children }: { style?: StyleProp<ViewStyle>; children?: ReactNode }) {
  const responder = useMemo(
    () =>
      PanResponder.create({
        onStartShouldSetPanResponder: () => Keyboard.isVisible(),
        onMoveShouldSetPanResponder: (_, { dx, dy }) => Keyboard.isVisible() && dy > TAP && dy > Math.abs(dx),
        onPanResponderRelease: (_, { dx, dy }) => {
          if (dy > TAP || (Math.abs(dx) < TAP && Math.abs(dy) < TAP)) Keyboard.dismiss();
        },
      }),
    [],
  );
  return (
    <View style={style} {...responder.panHandlers}>
      {children}
    </View>
  );
}
