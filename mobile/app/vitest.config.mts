import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

const here = (path: string) => fileURLToPath(new URL(path, import.meta.url));

// React Native and the native modules only run on a phone, so tests get stand-ins from test/mocks.
const mocked = ['react-native', '@sikemux/native', 'expo-clipboard', 'expo-device', 'expo-file-system', 'expo-router', 'expo-secure-store'];

export default defineConfig({
  resolve: {
    alias: [
      { find: /^@\//, replacement: here('./src/') },
      { find: /^@mac\//, replacement: here('../../src/') },
      ...mocked.map((name) => ({
        find: new RegExp(`^${name}$`),
        replacement: here(`./test/mocks/${name.replace('@', '').replace('/', '-')}.ts`),
      })),
    ],
  },
  test: {
    include: ['src/**/*.test.{ts,tsx}', 'test/**/*.test.{ts,tsx}'],
    environment: 'node',
    restoreMocks: true,
  },
});
