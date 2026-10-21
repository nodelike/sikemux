const fs = require('node:fs');
const path = require('node:path');
const { getDefaultConfig } = require('expo/metro-config');

const config = getDefaultConfig(__dirname);

// The chat transcript comes from the Mac app's own reducer (src/chat), so the
// phone reads an agent's updates exactly as the Mac does.
config.watchFolders = [...config.watchFolders, path.resolve(__dirname, '../../src')];
// The accounts API's types, client and close codes come from the server's protocol (@protocol).
config.watchFolders.push(path.resolve(__dirname, '../../server/protocol/generated'), path.resolve(__dirname, '../../server/protocol/src'));

// The pretend Mac's answers are read by the core's Rust parser, as a real host reads them.
let parser;
function readMarkdown(texts) {
  if (!parser) {
    const child = require('node:child_process').spawn(
      'cargo',
      [
        'run',
        '--quiet',
        '--manifest-path',
        path.resolve(__dirname, '../../src-tauri/Cargo.toml'),
        '-p',
        'sikemux-markdown',
        '--example',
        'stdio',
      ],
      { stdio: ['pipe', 'pipe', 'inherit'] },
    );
    const waiting = [];
    require('node:readline')
      .createInterface({ input: child.stdout })
      .on('line', (line) => waiting.shift()?.(JSON.parse(line)));
    parser = { child, waiting };
  }
  return Promise.all(
    texts.map(
      (text) =>
        new Promise((resolve) => {
          parser.waiting.push(resolve);
          parser.child.stdin.write(`${JSON.stringify({ text, options: { gfm: true, fileLinks: true } })}\n`);
        }),
    ),
  );
}

// `pnpm showcase` runs this app in a browser against a pretend Mac, for screenshots.
if (process.env.SIKEMUX_SHOWCASE) {
  const showcase = path.resolve(__dirname, '../showcase');
  const mocks = path.resolve(__dirname, 'test/mocks');
  const standIns = {
    '@sikemux/native': path.join(showcase, 'native.ts'),
    'expo-file-system': path.join(mocks, 'expo-file-system.ts'),
    'expo-secure-store': path.join(mocks, 'expo-secure-store.ts'),
    'expo-device': path.join(showcase, 'expo-device.ts'),
    'expo-media-library': path.join(showcase, 'expo-media-library.ts'),
    'expo-notifications': path.join(mocks, 'expo-notifications.ts'),
    '@react-native-masked-view/masked-view': path.join(showcase, 'masked-view.tsx'),
    '@clerk/expo': path.join(showcase, 'clerk.ts'),
    '@clerk/expo/apple': path.join(showcase, 'clerk.ts'),
    '@clerk/expo/google': path.join(showcase, 'clerk.ts'),
    '@clerk/expo/resource-cache': path.join(showcase, 'clerk.ts'),
  };
  config.watchFolders.push(showcase);
  const resolve = config.resolver.resolveRequest;
  config.resolver.resolveRequest = (context, name, platform) =>
    standIns[name] ? { type: 'sourceFile', filePath: standIns[name] } : (resolve ?? context.resolveRequest)(context, name, platform);

  const picture = process.env.SIKEMUX_SHOWCASE_PANE;
  config.server = {
    ...config.server,
    enhanceMiddleware: (middleware) => (request, response, next) => {
      if (request.url === '/showcase/markdown') {
        let body = '';
        request.on('data', (part) => (body += part));
        request.on('end', () =>
          readMarkdown(JSON.parse(body)).then((trees) => {
            response.setHeader('Content-Type', 'application/json');
            response.end(JSON.stringify(trees));
          }),
        );
        return;
      }
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
