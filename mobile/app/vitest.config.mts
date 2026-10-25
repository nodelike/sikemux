import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

const here = (path: string) => fileURLToPath(new URL(path, import.meta.url));

// React Native and the native modules only run on a phone, so tests get stand-ins from test/mocks.
const mocked = [
  'react-native',
  '@sikemux/native',
  '@react-native-community/netinfo',
  'expo',
  'expo-apple-authentication',
  'expo-application',
  'expo-clipboard',
  'expo-crypto',
  'expo-device',
  'expo-file-system',
  'expo-notifications',
  'expo-router',
  'expo-secure-store',
];

export default defineConfig({
  resolve: {
    alias: [
      { find: /^@\//, replacement: here('./src/') },
      { find: /^@shared\//, replacement: here('../../shared/') },
      { find: /^@protocol$/, replacement: here('../../server/protocol/generated/types.ts') },
      { find: /^@protocol\//, replacement: here('../../server/protocol/src/') },
      ...mocked.map((name) => ({
        find: new RegExp(`^${name}$`),
        replacement: here(`./test/mocks/${name.replace('@', '').replace('/', '-')}.ts`),
      })),
    ],
    dedupe: ['react', 'react-dom', 'vitest', '@testing-library/react'],
  },
  test: {
    include: ['src/**/*.test.{ts,tsx}', 'test/**/*.test.{ts,tsx}', '../../shared/**/*.test.{ts,tsx}'],
    environment: 'node',
    restoreMocks: true,
  },
});
