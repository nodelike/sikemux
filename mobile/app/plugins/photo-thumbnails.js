const { withAppDelegate } = require('expo/config-plugins');

// expo-media-library draws ph:// photos through an image loader that React Native finds by name in a list
// its codegen writes, but that list leaves out Expo modules, so the gallery's thumbnails would stay blank.
const PROVIDER = 'delegate.dependencyProvider = RCTAppDependencyProvider()';

const LOADERS = `
class PhotoThumbnailsDependencyProvider: RCTAppDependencyProvider {
  override func imageURLLoaderClassNames() -> [String] {
    super.imageURLLoaderClassNames() + ["MediaLibraryImageLoader"]
  }
}
`;

module.exports = (config) =>
  withAppDelegate(config, (config) => {
    const source = config.modResults.contents;
    if (!source.includes(PROVIDER))
      throw new Error('photo-thumbnails: the AppDelegate template changed; its dependency provider was not found');
    config.modResults.contents = source.replace(PROVIDER, 'delegate.dependencyProvider = PhotoThumbnailsDependencyProvider()') + LOADERS;
    return config;
  });
