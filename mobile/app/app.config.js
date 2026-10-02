/**
 * Builds are "Sikemux Dev" unless APP_VARIANT=production. The two install side by side,
 * each with its own key and paired Macs.
 */
module.exports = ({ config }) => {
  if (process.env.APP_VARIANT === 'production') return config;
  return {
    ...config,
    name: 'Sikemux Dev',
    scheme: 'sikemux-dev',
    ios: {
      ...config.ios,
      bundleIdentifier: `${config.ios.bundleIdentifier}.dev`,
      icon: './assets/dev.icon',
    },
    android: {
      ...config.android,
      package: `${config.android.package}.dev`,
      adaptiveIcon: {
        ...config.android.adaptiveIcon,
        backgroundColor: '#140c2a',
        backgroundImage: './assets/images/android-icon-background-dev.png',
      },
    },
  };
};
