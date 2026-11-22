# Agentero Android credential backend patch

This is the published [android-native-keyring-store 1.0.0](https://crates.io/crates/android-native-keyring-store/1.0.0) from [open-source-cooperative/android-native-keyring-store](https://github.com/open-source-cooperative/android-native-keyring-store), VCS commit `7d8164fb1346b86ece49dfd9e511078bc9151a13`. Downloaded crate SHA-256: `48c6349ddff23194f8fdce2ea8849380f5a4868c1648965b70e801e104cba9b3`. The original [MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE) licenses and published sources are retained.

The Android branch selects this copy through the workspace [Cargo patch](../../Cargo.toml). Its changes are limited to native credential persistence:

- New AES/GCM keys explicitly request 256 bits with Android's `setKeySize(256)`. The application uses the fresh `agentero-mobile-v2` vault and `keyring-agentero-mobile-v2` preferences/alias, rather than reusing the upstream default vault's existing 128-bit keys.
- A false `SharedPreferences.Editor.commit()` result returns `PersistenceFailure`, mapped to `keyring_core::Error::PlatformFailure`. It must not become `NoEntry` or `NoStorageAccess`/Locked.
- Idempotent deletion still performs remove and commit when the cached entry is missing. A previous failed delete can already have changed the in-memory map without saving it.
- Malformed Base64 is a corruption error. It is never treated as a missing record and its raw value is not logged.

The application credential facade serializes operations for this one underlying vault. Failed mutations quarantine reads until a successful explicit write/remove confirms persistence. Locked results preserve prior quarantine. This matters because Android updates the SharedPreferences memory map before disk completion; returning an error alone does not undo that update. The behavior was checked against pinned [AOSP SharedPreferencesImpl](https://android.googlesource.com/platform/frameworks/base/+/9cdc7993c7f6f5d9634565c8f39169e18758f700/core/java/android/app/SharedPreferencesImpl.java).

[Android data extraction rules](../../src-tauri/gen/android/app/src/main/res/xml/data_extraction_rules.xml) exclude this preferences file from cloud backup and device transfer. Keystore keys cannot be restored alongside ciphertext on another installation. The legacy Bridge credential directory is also excluded while migration remains pending.

This crate uses Rust JNI and Tauri's initialized application context, without Kotlin/Gradle additions or a separately loaded JNI library. Do not initialize ndk-context a second time. Credential calls run after native startup on blocking worker threads; no generic credential commands or export capabilities are registered in the WebView.

Validation includes two host tests for commit failure classification and corrupt encoding, the application facade's cached-failure/retry regression, and Android ARM64 compilation. These checks do not establish real-device hardware backing, power-loss behavior, or background service lifecycle acceptance. Existing keys are not silently upgraded; the fresh namespace is the migration boundary.
