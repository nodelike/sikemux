const { defineConfig } = require('eslint/config');
const expoConfig = require('eslint-config-expo/flat');

module.exports = defineConfig([
  expoConfig,
  {
    ignores: ['ios/', 'android/', '.expo/', 'src/**/*.generated.ts'],
  },
  {
    // eslint-plugin-react cannot detect the React version under ESLint 10.
    settings: { react: { version: '19.2' } },
  },
  {
    files: ['plugins/**/*.js', '*.config.js'],
    languageOptions: { globals: { __dirname: 'readonly', require: 'readonly', module: 'writable', process: 'readonly' } },
  },
]);
