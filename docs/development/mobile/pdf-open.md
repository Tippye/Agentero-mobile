# Android 系统打开与分享 PDF

Android 分支接入系统「用 Agentero 打开」与分享菜单中的单份 PDF 入口，将系统授权文档导入当前本地文献库。冷启动通过插件 `load` 读取一次启动 Intent，已运行时通过公开 `Plugin.onNewIntent` 接收；不覆盖 Activity 的 Wry/Tauri Intent 分派，也不借用仅支持 `agentero:` 的配对深链处理器。

## 原生接收边界

- 清单为打开与分享分别注册独立过滤器：`ACTION_VIEW` 使用 `content` scheme 与 `application/pdf`；`ACTION_SEND` 只匹配 PDF MIME 与 DEFAULT category，不添加 scheme 或 BROWSABLE，因为分享 URI 通常放在 `EXTRA_STREAM`，`Intent.data` 为空。MIME 过滤器不是授权边界，即使某个 file URI 能通过系统路由，运行时也只接受 `content://`。不注册 `SEND_MULTIPLE`、文本或通配 MIME。
- MIME 只用作路由，不能证明文件内容是 PDF。VIEW 只接受一份 `Intent.data` 且拒绝任何 `EXTRA_STREAM`；SEND 必须有单个、成功解码为 Uri 的 `EXTRA_STREAM`，可选 data 只能为空或与 stream 完全一致。缺失、错误类型、数组/列表或解码异常不回退为 data，也不解释为多文件。
- ClipData 可以为空，或只有同一个被选 URI。不同 URI、多项、文字、HTML 和嵌套 Intent 均拒绝。SEND 从一个已知 extra 读取类型化 Uri，使用现有 AndroidX Core 1.17.0 的 `IntentCompat.getParcelableExtra`：它在 API 34+ 使用平台类型化接口，在 API 33 避开已知平台缺陷并做实例类型检查。不自行强转，不遍历/转发 extras；异常路径保留现有拒绝边界。
- 必须有 `FLAG_GRANT_READ_URI_PERMISSION` 且当前进程实际获得读取授权。URI 长度有界，必须为合法 `content://`，拒绝解码后带用户前缀的 authority；不会把 URI 变为本地文件路径。
- 原生邮箱最多保留一个待处理 URI。第一份保留，后续有效请求显示英中本地化的忙碌提示，用户可在处理后重新打开。已运行的 Intent 先到时不会再重放旧启动请求；清空和销毁不会重新读取已消费的冷启动 Intent。
- URI 只留在原生内存中，不发送到前端、不写日志或磁盘、不请求持久授权。读取权限仍受 Android Activity/task 与提供器生命周期限制；长时间挂起或关闭之后没有延迟读取保证，需要重新打开。

## 当前文献库与资源所有权

前端先注册不含文档信息的 `mobile:external-pdf-pending` 唤醒事件，再读取当前待处理项。事件、前台聚焦和可见性变化只请求一次检查，不使用轮询；没有文献库、页面隐藏或已有导入占用时不领取。事件注册迟到、卸载和切换文献库均按当前已提交的挂载/文献库 epoch 隔离；手动选择器、浏览器选择与系统打开/分享共享准入锁，忙碌时合并一个唤醒，在当前操作结束后检查。

`paper_import_opened_pdf(vaultPath)` 不接收 URI、路径、FD 或请求正文，只允许主应用窗口提交当前本地文献库。桌面返回 null；iOS 从自己的原生 Inbox 描述符邮箱领取，见[iOS 系统复制 PDF 入库](ios-pdf-open.md)，不调用 Android 插件。Android 的 Rust 注册一次原生唤醒通道后再领取邮箱；原生待处理项在领取时从邮箱移除，随后才打开 descriptor，读取失败不会在每次聚焦时自动重试同一来源。

Rust 在领取前捕获规范 Vault 路径与目录身份，并取得与选择器相同的独占许可。现有私有 opener 在打开和查询名称前验证精确 provider，拒绝未知 provider、本应用/共享 UID 与跨资料用户前缀；使用只读 `openFileDescriptor`，保留完整文件/pipe 语义，不丢弃 AssetFileDescriptor 的子区间。提供器 I/O 在现有有界线程池执行；FD 转交继续使用既有 detach/resolve 所有权规则，返回后由 Rust `File` 管理。

空邮箱不会创建暂存目录或启动后台工作。真正领取到文件后，单个宿主拥有者保留 FD、许可、临时目录和既有后台 transfer 租约直到复制与入库真实完成；调用页面销毁不提前删除仍在复制的文件，也不把已开始的入库宣称为取消。暂存继续复用原生选择器的 64KiB 分块、前 1024 字节 PDF 标头检查、严格 256MiB 单文件与 1GiB 批次实际读取预算，不整份读入 WebView、不要求 Seek。显示名只作为受限文件名；提供器名称中的不成对 UTF-16 surrogate 会在原生 JSON/FD 返回前被省略，合法 emoji 配对保留，避免可选元数据破坏 JNI 解码。标题继续由原有入库/后续识别策略处理。

暂存前及提交前再次核对 Vault 身份，复用同一个导入内核、去重、Catalog 写入与准备计划。旧页面的迟到结果不刷新新文献库或弹出错误。外部 PDF 成功入库后只刷新文献库，不自动切走正在阅读/编辑的文档，不替换未保存草稿，也不打开 Agent 侧栏；用户从文献库选择论文阅读。该行为不等于多窗口文档打开，也未增加 Agent 请求或未知远程操作重放。

## 出站 FileProvider 的目录边界

Android 的非导出 FileProvider 保留临时 URI 授权能力，但路径映射不再包含整个外部存储或整个私有 cache，只允许两个明确子目录：

- `external-files-path` 的 `Pictures/`：固定 Wry 0.57.1 的 HTML 文件选择器相机捕获会通过 `getExternalFilesDir(Environment.DIRECTORY_PICTURES)` 创建临时 JPEG，再调用本应用的 FileProvider。保留这个应用专属目录，使 SDK 捕获流程仍有合法映射；它不是公共相册或外部存储根。该目录不可用或捕获创建失败时，固定 SDK 会退回普通文件选择；真实设备的捕获与回传仍需验收。
- `cache-path` 的 `exports/`：为显式生成的出站副本预留，不能把 Vault 原件、导入暂存、凭据或其他 cache 文件直接当作可共享路径。本轮只限定映射，没有新增对外分享按钮、复制器或自动 URI 授权。

Provider 仍为 `exported=false`、`grantUriPermissions=true`，authority 与系统入口均不变；路径映射本身不是公开访问授权，也不据此宣称之前发生过数据泄漏。实际出站操作仍需给指定目标最小临时权限，并遵循明确的副本所有权与清理规则。目录外路径和遍历由成熟 AndroidX FileProvider 的规范路径边界拒绝，不另写路径解析器。

当前批注 PDF 的“另存为”使用系统文档创建器和 `plugin-fs` 的 SAF URI，不依赖 FileProvider，因此继续保留其目标授权及原件保护。入站 PDF 仍拒绝本应用/共享 UID 的提供器；收紧出站映射不扩大入站权限。

目录收敛阶段的 29 项 PDF 另存为、笔迹导出与原生入库回归通过。验证还会检查最终 APK 内编译后的路径 XML 与 provider 声明；这些结果不等于真实相机授权、拍照回传或目标应用读取验收。

## 验证边界

原生纯 JVM 策略检查覆盖动作/MIME、URI 与授权、相同/不同 ClipData、文本拒绝、VIEW 多余 stream 拒绝与 SEND 缺失/冲突 stream 的选择规则，以及邮箱的单次领取、容量、清理与冷暖启动顺序。它们不模拟 Binder 授权或提供器。Rust 复用的标头、流式上限、私有暂存及 Vault 身份检查继续回归；新增拥有者检查在等待点丢弃调用接收者，确认真实操作结束前许可与临时目录仍有效。事件契约断言唤醒序列化为 null，不含文档字段。

前端检查覆盖监听先于领取、空邮箱无重复请求、共享准入锁、隐藏/忙碌时唤醒合并、卸载和旧文献库迟到结果隔离。打开入口接入阶段，16 项前端交接检查通过；移动端/PDF 回归共 1262 项通过、10 项既有跳过，TypeScript 与依赖图（1478 模块、8116 边）通过。Rust 插件 4 项、原生暂存/所有权 10 项、事件契约 10 项、移动命令登记及严格 workspace/all-targets Clippy 通过，绑定已生成；随后宿主全套 929 项中 919 通过、10 项既有外部环境用例忽略，包含绑定只读比对与事件契约。Android 插件与应用 Kotlin 重新编译、58 项 JVM 检查通过。完整端到端还需设备验证：文件管理器/邮件/云提供器的读取授权、冷启动、已在前台时再次打开、拒绝与取消、任务被关闭后的授权失效，以及实际入库后的文献库显示。当前无连接设备，不能以编译和纯策略检查替代这些验收。iOS 后续接入仅限本应用 Inbox 的单份系统副本；其原地打开、分享扩展和完整设备验收，以及 Android 多份/文本分享仍在后续范围。

单份分享阶段复用相同的 Rust/前端契约，没有新增 URI IPC 或改变文献库提交逻辑；41 项既有前端交接、选择器及工作区回归通过。插件重新编译、应用 Kotlin/清单处理检查及 61 项 JVM 检查通过；最后一次重编译已消除新增的冗余 null 检查警告。新增原生策略用例只验证选择规则，不实例化 Android Intent/Parcelable，也不证明真实反序列化或 Sharesheet 行为；这些路径仍需 Android 13 和更新系统上的设备验收。

参考：

- [Tauri Android Plugin 的公开生命周期](https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri/mobile/android/src/main/java/app/tauri/plugin/Plugin.kt)
- [Tauri deep-link 插件的冷启动与 onNewIntent 处理](https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/deep-link/android/src/main/java/DeepLinkPlugin.kt)
- [Android 临时 URI 读取授权](https://developer.android.com/training/secure-file-sharing/share-file)
- [Android 官方窄范围 FileProvider 配置建议](https://developer.android.com/privacy-and-security/risks/file-providers)与 [FileProvider 路径类型](https://developer.android.com/reference/androidx/core/content/FileProvider)；[GitHub CodePath 的相机分享目录示例](https://github.com/codepath/android_guides/wiki/Sharing-Content-with-Intents)用于对照 `getExternalFilesDir(Pictures)` 与 `external-files-path`。实际兼容路径以固定 [Wry 相机捕获实现](https://github.com/tauri-apps/wry/blob/wry-v0.57.1/src/android/kotlin/RustWebChromeClient.kt)为准。
- [Android ACTION_VIEW](https://developer.android.com/reference/android/content/Intent#ACTION_VIEW)
- [Android 接收分享与 IntentCompat 推荐接口](https://developer.android.com/develop/ui/compose/sharing/receive)、[MIME-only extra 分享过滤器](https://developer.android.com/training/basics/intents/filters)及 [IntentCompat](https://developer.android.com/reference/androidx/core/content/IntentCompat#getParcelableExtra(android.content.Intent,java.lang.String,java.lang.Class%3CT%3E))；已核对固定 Core 1.17.0 源码的 API 33 兼容路径。
- [Readest 的单份/多份分享分派](https://github.com/readest/readest/blob/main/apps/readest-app/src-tauri/gen/android/app/src/main/java/com/bilingify/readest/MainActivity.kt)与 [MIME-only 分享过滤器](https://github.com/readest/readest/blob/main/apps/readest-app/src-tauri/gen/android/app/src/main/AndroidManifest.xml)：只参考系统 transport 形态，保留本应用的单份 PDF、原生 URI 所有权与授权边界。
