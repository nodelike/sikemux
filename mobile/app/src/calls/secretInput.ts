import type { TextInputProps } from 'react-native';

/** A field a secret is typed into: the keyboard neither corrects, suggests, fills nor remembers what goes in. */
export const SECRET_INPUT = {
  autoCorrect: false,
  spellCheck: false,
  autoCapitalize: 'none',
  autoComplete: 'off',
  importantForAutofill: 'no',
  keyboardType: 'visible-password',
} as const satisfies TextInputProps;
