const fs = require('node:fs');
const path = require('node:path');
const { getDefaultConfig } = require('expo/metro-config');

const config = getDefaultConfig(__dirname);

// The chat transcript comes from the Mac app's own reducer (src/chat), so the
// phone reads an agent's updates exactly as the Mac does.
config.watchFolders = [...config.watchFolders, path.resolve(__dirname, '../../src')];

// `pnpm showcase` runs this app in a browser against a pretend Mac, for screenshots.
if (process.env.SIKEMUX_SHOWCASE) {
  const showcase = path.resolve(__dirname, '../showcase');
  const mocks = path.resolve(__dirname, 'test/mocks');
  const standIns = {
    '@sikemux/native': path.join(showcase, 'native.ts'),
    'expo-file-system': path.join(mocks, 'expo-file-system.ts'),
    'expo-secure-store': path.join(mocks, 'expo-secure-store.ts'),
    'expo-device': path.join(showcase, 'expo-device.ts'),
    'expo-notifications': path.join(mocks, 'expo-notifications.ts'),
    '@react-native-masked-view/masked-view': path.join(showcase, 'masked-view.tsx'),
    '@clerk/expo': path.join(showcase, 'clerk.ts'),
    '@clerk/expo/apple': path.join(showcase, 'clerk.ts'),
    '@clerk/expo/google': path.join(showcase, 'clerk.ts'),
    '@clerk/expo/resource-cache': path.join(showcase, 'clerk.ts'),
    '@clerk/expo/token-cache': path.join(showcase, 'clerk.ts'),
  };
  config.watchFolders.push(showcase);
  const resolve = config.resolver.resolveRequest;
  config.resolver.resolveRequest = (context, name, platform) =>
    standIns[name] ? { type: 'sourceFile', filePath: standIns[name] } : (resolve ?? context.resolveRequest)(context, name, platform);

  const picture = process.env.SIKEMUX_SHOWCASE_PANE;
  config.server = {
    ...config.server,
    enhanceMiddleware: (middleware) => (request, response, next) => {
      if (picture && request.url === '/showcase/pane.jpg') {
        response.setHeader('Content-Type', 'image/jpeg');
        fs.createReadStream(picture).pipe(response);
        return;
      }
      return middleware(request, response, next);
    },
  };
}

module.exports = config;
