const { AndroidConfig, withAndroidManifest } = require('expo/config-plugins');

const SHARED_TYPES = ['text/plain', 'image/*', 'video/*', '*/*'];

// Another app's share menu opens the app on its share screen, for text, links, pictures, videos and any file.
module.exports = (config) =>
  withAndroidManifest(config, (config) => {
    const activity = AndroidConfig.Manifest.getMainActivityOrThrow(config.modResults);
    const filters = (activity['intent-filter'] ??= []);
    const sharing = (filter) => filter.action?.some((action) => action.$['android:name'].startsWith('android.intent.action.SEND'));
    activity['intent-filter'] = filters.filter((filter) => !sharing(filter));
    for (const action of ['android.intent.action.SEND', 'android.intent.action.SEND_MULTIPLE']) {
      activity['intent-filter'].push({
        action: [{ $: { 'android:name': action } }],
        category: [{ $: { 'android:name': 'android.intent.category.DEFAULT' } }],
        data: SHARED_TYPES.map((type) => ({ $: { 'android:mimeType': type } })),
      });
    }
    return config;
  });
