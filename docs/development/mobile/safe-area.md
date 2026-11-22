# Mobile system bars and safe areas

Android and WebKit each have one inset owner. Android fits the native WebView viewport; iOS/browser viewport-edge surfaces use CSS safe areas. Inner headers, navigation, editor content and footers do not add system insets again.

## Android 13+ / target SDK 36

`MainActivity` retains AndroidX `enableEdgeToEdge()` and explicitly keeps status/navigation bars visible. It uses the existing AndroidX `ViewCompat` / `WindowInsetsCompat` APIs rather than a JavaScript inset bridge or a fullscreen flag.

Wry 0.57.1 calls `setWebView` (and `onWebViewCreate`) before the eventual JNI `setContentView(View)` call in `android/main_pipe.rs`. Installing a listener only in `onCreate` or `onWebViewCreate` can miss that content installation. The Activity overrides `setContentView(View)` and its layout-parameter overload, calls the superclass, then installs the listener on `android.R.id.content`.

The containing FrameLayout receives padding from the union of `systemBars() | displayCutout() | ime()`. Android's union takes the maximum on each edge: the keyboard and navigation bar are not summed. This handles a bottom gesture bar, landscape left/right three-button bars, display cutouts and keyboard changes. Each delivery replaces padding with the latest absolute values, so rotation and repeated dispatch cannot accumulate padding. `SOFT_INPUT_ADJUST_RESIZE` enables compatible IME inset delivery.

The listener consumes those insets after fitting the native content. The child WebView has a genuinely smaller safe viewport and must not consume the same obstruction a second time. Android pixel values stay native; no density conversion, CSS pixel constant, WebView translation or synthetic pointer offset is introduced. Existing web layout and PDF coordinates continue to use their actual viewport.

The Activity requests initial dispatch once the content is attached; normal Android inset delivery handles subsequent rotation, navigation-mode and IME changes. No generated Wry/Tauri source or SDK dependency is patched.

## Shared CSS contract

The viewport meta tag uses `viewport-fit=cover` so WebKit can expose the safe region through `env(safe-area-inset-*, 0px)`. The default fallback is zero in ordinary browser windows and native-fitted Android views.

`src/index.css` defines:

- `--mobile-safe-top`, `--mobile-safe-right`, `--mobile-safe-bottom`, `--mobile-safe-left`: browser CSS pixel safe insets.
- `.mobile-safe-area`: border-box padding on all four edges, using the maximum of the inset and the corresponding `--mobile-safe-gutter-{top,right,bottom,left}`. Gutters default to zero and retain a surface's normal visual spacing.

Apply the class only to hosts whose edges reach the viewport: the mobile application root, a full-screen pairing/scanner surface, the sync dialog portal, or the sidebar drawer. Do not apply it again to an inner header, navigation, scroll pane or footer. A fixed/absolute overlay may still reach the viewport even when its DOM is nested inside a padded application shell, so there is no blanket descendant rule that zeros safe areas. Each viewport-edge surface owns its own insets exactly once.

Pairing/scanning uses gutter custom properties instead of inline `env()` expressions. The sync dialog owns the safe area; its close row and scrolling footer retain ordinary spacing. Header/navigation remain normal-flow children of an already fitted host.

## References and verification

This uses the public patterns from [Android's WindowInsetsAnimation sample](https://github.com/android/user-interface-samples/blob/main/WindowInsetsAnimation/app/src/main/java/com/google/android/samples/insetsanimation/MainActivity.kt), [AndroidX ViewCompat](https://github.com/androidx/androidx/blob/androidx-main/core/core/src/main/java/androidx/core/view/ViewCompat.java), and [WebKit's safe-area guidance](https://webkit.org/blog/7929/designing-websites-for-iphone-x/). Safe-area `env()` values are combined with existing design gutters through `max()`, as WebKit recommends.

Three focused frontend regressions cover the four-edge CSS contract and real pairing/scanner/sync DOM host ownership; they pass together with nine workspace/gesture regressions (12 total). Native `:app:compileArm64DebugKotlin` passed against SDK 36 with `:app:rustBuildArm64Debug` excluded; this checks the actual AndroidX nullable override signatures and inset APIs without rebuilding Rust. Normal upstream WebView/back-navigation deprecations and the explicit adjust-resize compatibility flag remain warnings. These checks do not establish device geometry: Android 13–16 gesture/three-button navigation, landscape cutouts, rotation, IME opening/closing, and iOS home-indicator layouts require physical device or emulator verification. In particular, confirm that header controls and the last scrollable row stay inside visible system bars, keyboard focus remains reachable, and PDF pointer/selection coordinates align after the viewport changes.
