# iOS 内容进程恢复

WebKit 可以因内存压力等原因结束独立的网页内容进程；WKWebView、Swift 插件和 Rust 宿主未必同时结束。固定版本 Tauri 2.12.5 / tauri-runtime-wry 2.12.3 已有默认自动重载，但默认路径没有重试上限。本模块使用公开的 `Builder::on_web_content_process_terminate` 替换默认行为，补上有界重载和旧手写目标失效。

该 API 只支持 macOS/iOS；产品接入仅在 iOS 编译启用，不把它误当作 `WebviewEvent` 枚举，也不声明已覆盖 Android renderer-gone。无需替换 WKNavigationDelegate 或修改 SDK。

## 恢复范围与上限

- 当前移动应用只有一个主 WebView，因此仅处理 `main`。未来多 Scene/多窗口需要独立的创建、身份与恢复策略，不能直接扩展标签白名单。
- 仅允许应用入口：`tauri://localhost`，无用户名、密码或端口，路径为空、`/` 或 `/index.html`；保留 SPA query/hash。开发构建还允许配置中精确的 dev URL scheme、host、有效端口与入口路径，不把任意 localhost、私网或 HTTP 地址视为应用页面。
- URL 不可读取或已导航到其他来源时不重载，不导航到猜测的兜底地址。自定义 scheme 不能用可能返回 `null` 的 origin 字符串比较。
- 每个宿主恢复预算在以首次尝试起算的固定 60 秒窗口内最多计入两次 reload 调度尝试，调度失败也计入。页面 `Finished`、URL 变化和相同标签重建都不清零预算，避免瞬间加载完成之后继续崩溃形成循环。
- 预算耗尽后停止本次自动恢复，不安排定时重试，不退出或重启整个应用。后续真实终止事件只能在预算时间窗允许时再尝试；当前页面仍不可用时需用户关闭重开。整个宿主重启会重新建立预算。
- 放开策略锁后直接调用回调所给 WebView 的公开 `reload()`，沿用其 runtime window/webview ID，不另外按标签查找或排队重载。上游全局 hook 自身会按标签取回 WebView，这一 SDK 行为并未在本模块中改写；当前单主窗口范围不代表已证明未来标签复用/多 Scene 的完整所有权。

`reload()` 成功只表示 SDK 接受了调度，不能表示 React 初始化成功、内存压力消失或论文已经恢复。原生日志只记录通用结果，不输出 URL、论文路径、token 或进程详情。

## 手写目标与宿主操作

每次主页面内容进程终止都会在来源/预算决策与重载前同步撤销该窗口的 Rust 手写目标，即使 URL 不允许重载或预算已经耗尽。SDK URL 属性在此之前读取，但读取不导航或重载页面。只处理匹配的活动目标，不碰其他窗口；旧 active ACK 不能把已经失效的目标重新确认为可分派。

随后以单一拥有者异步发送原有 `stylusSetTarget(active:false)`，期限五秒；不阻塞 WebKit 回调。Native 清理失败、缺少驱动或 generation 耗尽均不伪造成功 ACK，Rust 仍拒绝旧 token 的动作。连续终止对同一 inactive 目标合并清理，不累积任务。新页面需要重新注册事件监听、创建新 token 并取得原生 ACK；旧清理的 generation/token 不会移除新目标。

Rust 持有的远程 journal、任务和已配对传输不因网页进程终止而清空；本模块不调用 Agent 发送、恢复、重新配对、凭据轮换或未知操作重放。页面启动后沿用已有订阅与读取流程。已经开始的宿主原生导入遵循自己的资源所有权，不以网页重载冒充已取消或回滚。

## 数据恢复边界

页面的 JS 变量、未保存表单和前端频道可能消失。阅读位置使用已经确认落盘的按窗口路由；笔记、笔迹只能恢复到已保存或已确认的内容。进程回收前尚未提交的内存草稿没有恢复保证，不能把“自动保存存在”写成“绝不丢失最近输入”。不会为了显示恢复页面自动打开 Agent 侧栏或重新发送草稿。

## 验证

纯策略检查覆盖精确应用来源、未知 URL/其他视图拒绝、两次预算及边界、失败计数、先失效再重载、预算耗尽后仍失效，以及原生 dispatcher 前释放锁。macOS 单元测试只构造 Builder 而不创建 runtime/window，实际编译公开 hook 与应用 glue；生产 macOS 不安装此行为。Linux/Windows 测试不引用平台不支持的 hook。

手写 slot 用阻塞 ACK 检查终止时的即时失效、无关窗口与重复通知合并、generation 耗尽后迟到 active ACK 不复活/不清除清理权，以及旧 ACK/清理不能移除新目标。本轮 5 项恢复检查、8 项手写路由检查、绑定只读比对、9 项事件契约和 workspace/all-targets 严格 Clippy 均通过。Swift delegate、Android 输入及既有远程消息协议均不在本轮改写。

当前环境没有完整 Xcode/iPhoneOS SDK，也没有实际触发 iPadOS 进程回收。宿主测试和 Rust 公共 API 编译不能代替真机内存压力、页面重建、事件重订阅与最后确认保存内容的端到端验收。

参考：

- [Tauri Builder 公开终止 hook](https://docs.rs/tauri/2.12.5/tauri/struct.Builder.html#method.on_web_content_process_terminate)
- [Apple WebKit 内容进程终止回调](https://developer.apple.com/documentation/webkit/wknavigationdelegate/webviewwebcontentprocessdidterminate(_:))
- [Firefox iOS 的有界崩溃恢复](https://github.com/mozilla-mobile/firefox-ios/blob/main/firefox-ios/Client/Frontend/Browser/BrowserViewController/Extensions/BrowserViewController%2BWebViewDelegates.swift)
- [Capacitor 的 bridge 重置与页面重载](https://github.com/ionic-team/capacitor/blob/main/ios/Capacitor/Capacitor/WebViewDelegationHandler.swift)
