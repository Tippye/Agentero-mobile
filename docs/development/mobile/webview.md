# Android WebView 兼容性

Tailwind CSS 4 需要 Chromium 111 或更新内核。Android 13 的初始系统 WebView 可能低于这个版本；用 React 内的提示无法保证用户能看清操作，因此 Android 分支在首次应用 URL 加载前使用原生保护页。

## 启动边界

保留 Tauri / Wry 的完整 `super.onCreate` 与 `super.onWebViewCreate`。Wry 0.57.1 的公开 `onWebViewCreate` 回调在首次 `loadUrl` 前执行；这里只检查内核并在已知不支持时关闭该 WebView 的页面 JavaScript，不跳过 SDK 生命周期，不更换 WebViewClient，也不修改生成的 SDK 源码。SDK 仍可注册 document-start 脚本或经原生 `evaluateJavascript` 初始化；这里不声明完全没有 JavaScript 执行，也不是脚本安全隔离机制。

MainActivity 的两个 `setContentView` 重载只在「已知不支持 + 输入视图是 WebView」时换成原生 ScrollView、文字和按钮，其他视图原样传给 SDK。原有系统栏、刘海和键盘 inset 处理继续作用于实际内容容器。这个入口是在 React 加载前保护界面，**不是**在 WebView 或 Rust 运行时创建前拦截，也不覆盖 WebView 提供器缺失导致的构造失败。

## 版本判定

- 在当前 WebView 已创建的回调中读取公开 `WebSettings.getDefaultUserAgent`，检查实际内核的单个完整 `Chrome/major.minor.build.patch` token。
- 已知 major 小于 111 才阻止前端；111 及以上保留 SDK 内容，不主动重新启用原本关闭的 JavaScript。
- 空值、异常、非 Chrome、重复 token、零值、整数溢出及格式不明均为 unknown，保留正常启动，不能把 unknown 宣称为已验证兼容。
- 不把 provider 的应用 `versionName` 当作 Chromium major。华为等厂商可使用自定义应用版本号，不能用其首段数字错误阻止可用设备。
- 默认 UA 在原生调用中解析后丢弃，不记录完整 UA、不发送 IPC 或遥测；不扫描已安装浏览器，不替换系统 WebView。

## 原生恢复页

保护页只展示当前/最低内核版本、更新后重新打开的说明，以及「打开设置」「关闭」两个操作。文字使用 Android 英文与简体中文资源，跟随 Android 资源语言；此时 React 的 `t()` 与应用内语言设置尚未加载。

Activity 的清单自行处理语言、字体比例、密度、布局方向和 uiMode 等配置变化，因此不能永久缓存最初创建的原生文字与主题。`onConfigurationChanged` 先交给 Tauri/AppCompat 更新配置，再仅重建已显示的兼容提示页：使用新的 `ContextThemeWrapper` 应用清单中的 DayNight 主题，重新读取当前资源、SP/DP 尺寸与背景。替换保留父容器、子视图位置和布局参数，原有安全区域监听器继续作用，并重新请求 insets；已脱离容器的缓存直接释放，下一次需要时才创建。

该刷新不重建 Activity、WebView 或 Rust，不重新查询内核、不改变 JavaScript、不重新加载论文。正常及 unknown 内核页面仍保留 SDK 视图。系统设置更新提供器之后仍需按提示关闭重开，配置变化不能当作内核已升级的证明。

只有用户按「打开设置」才查询当前实际 WebView provider，打开其应用详情；不可用时退回系统设置，再失败则使用本地化原生 Toast。不自动打开商店、下载组件或修改默认提供器，不新增权限。关闭只结束当前 Activity，不使用 `finishAffinity`、进程终止或 `app.exit`，不主动停止既有后台服务。

## 复用与验证

实现复用 Android Framework 的 WebSettings、WebView、原生控件与设置 Intent。Capacitor 的成熟检查提供了原生版本判定参考，但其 provider 版本号策略需处理厂商版本例外；这里改用实际内核默认 UA。动态替换/下载 WebView 提供器与新的启动 Activity 均不在此范围。

9 项纯 JVM 策略用例覆盖 Chrome 110/111 边界、当前内核、厂商默认 UA、重复/异常/溢出/缺失等情况；连同现有原生用例共 47 项通过。配置刷新接入后，插件及 MainActivity Kotlin 均重新编译通过，47 项既有 JVM 策略检查再次通过；这些策略检查不渲染原生 View，不将其计为配置变化的视觉验收。当前未连接 Android 设备，原生页面的真实字体缩放、无障碍、设置回跳、旋转和系统组件更新仍需真机验收；桌面 `pnpm tauri dev` 不会执行这段 Android UI，不据此声明已完成原生视觉验收。

参考：

- [Tailwind 浏览器兼容性](https://tailwindcss.com/docs/compatibility)
- [Activity 配置变化与资源更新](https://developer.android.com/reference/android/app/Activity#onConfigurationChanged(android.content.res.Configuration))；参考 [AndroidX AppCompat 的配置回调](https://github.com/androidx/androidx/blob/androidx-main/appcompat/appcompat/src/main/java/androidx/appcompat/app/AppCompatActivity.java)，实现前另核对了固定依赖 1.7.1 的源码。沿用成熟主题/资源更新机制，不自行覆盖全局资源配置或重写主题系统。
- [WebSettings.getDefaultUserAgent](https://developer.android.com/reference/android/webkit/WebSettings#getDefaultUserAgent(android.content.Context))
- [Wry 0.57.1 Activity 公共回调](https://github.com/tauri-apps/wry/blob/wry-v0.57.1/src/android/kotlin/WryActivity.kt)
- [Wry 首次导航与内容安装次序](https://github.com/tauri-apps/wry/blob/wry-v0.57.1/src/android/main_pipe.rs)
- [Capacitor 原生 provider 版本检查](https://github.com/ionic-team/capacitor/blob/main/android/capacitor/src/main/java/com/getcapacitor/Bridge.java)
