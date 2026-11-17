const { AndroidConfig, withAndroidManifest } = require('expo/config-plugins');
const { withNotificationsAndroid } = require('expo-notifications/plugin/build/withNotificationsAndroid');

const SCHEME_META = 'com.nodelike.sikemux.scheme';

// Cards from hosts are built on the phone by modules/notify, which opens their links with this build's own scheme.
module.exports = (config) => {
  config = withNotificationsAndroid(config, { icon: '../../brand/mark/mark-white-256.png', color: '#a277ff' });
  return withAndroidManifest(config, (config) => {
    const application = AndroidConfig.Manifest.getMainApplicationOrThrow(config.modResults);
    const scheme = Array.isArray(config.scheme) ? config.scheme[0] : config.scheme;
    AndroidConfig.Manifest.addMetaDataItemToMainApplication(application, SCHEME_META, scheme);
    return config;
  });
};
