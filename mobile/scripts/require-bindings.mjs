import { bindingsExist } from './native-build.mjs';

if (!bindingsExist()) {
  console.error('\nBuild the native bindings first: run `pnpm native:ios:sim` in mobile/.\n');
  process.exit(1);
}
