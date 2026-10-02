const { withXcodeProject } = require('expo/config-plugins');

// The Rust client reads the phone's proxy settings, and a static library cannot carry its own framework links.
module.exports = (config) =>
  withXcodeProject(config, (config) => {
    config.modResults.addFramework('SystemConfiguration.framework');
    return config;
  });
