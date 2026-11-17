import { bindingsExist } from './native-build.mjs';

if (!bindingsExist()) {
  console.error('\nBuild the native bindings first: run `pnpm native:android` in mobile/.\n');
  process.exit(1);
}
