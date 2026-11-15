# Mobile apps

The root `AGENTS.md` applies here too.

- The iPhone app is native SwiftUI in `mobile/ios`. `project.yml` is its XcodeGen spec
  (`xcodegen generate`; the `.xcodeproj` is build output), and
  `mobile/ios/scripts/build-core.sh` builds `crates/sikemux-mobile` into
  `build/SikemuxCore.xcframework` with its Swift bindings, made by the crate's own
  `uniffi-bindgen` bin behind its `bindgen` feature, which the phone never links. Run it
  before the first Xcode build and after any change to the Rust the phone links. The core's
  types and the phone's calls are generated into `Wire.generated.swift` by
  `scripts/generate-wire-types.mjs`, the icons into `PhoneIcons.generated.swift` by
  `scripts/generate-notch-icons.mjs`; never hand-edit either. The app compiles the notch's
  `IconView.swift` by path rather than copying it.
- The Android app is in `mobile/app` (Expo) with the core's Rust client bridged in
  `mobile/native`. `mobile/` is its own pnpm workspace: never add it to the root install,
  scripts or checks.
- TypeScript the Android app and the share page both use lives in `shared/`, imported as
  `@shared/*`. It is plain source, not a package: each workspace that imports it resolves
  its packages from its own install, and its tests run under the phone's vitest.
- Phone builds need rustup's Rust first on `PATH` as well; Homebrew's has no phone
  targets.
- Teammates do not boot simulators. The lead runs the app on one simulator after merging
  their work, always through `sim attach`, never `simctl boot` or Xcode.
- The bindings UniFFI and `uniffi-bindgen-react-native` generate are build output; do not
  commit or hand-edit them.
- A new phone screen is mocked up in `mobile/design/screens.src.html` before it is built; run
  `pnpm design` in `mobile/` to view it. Once a screen is built, its showcase pictures are the
  record, and the design file is not kept in step with it.
- sikemux.com's screenshots come from here. `pnpm shots` in the site runs the phone showcase
  (`mobile/showcase/`) and cuts the feature cards from the Mac pictures already in the
  site, which stay as they are until the GPUI app has a showcase of its own.
