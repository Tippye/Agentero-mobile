# 平板端实施方案：Android 篇

> 状态：**开发中，尚未完成真机验收**。现状分析与行号基于 `66ecc126`（v0.15.5，Tauri 2.12.5 / tao 0.37.2）。
> 两端共用的部分在 `mobile` 分支维护，见 [tablet-shared.md](tablet-shared.md)。`android` 基于 `mobile`，这里只写 Android 特有工作；文中 S0–S9 指共享篇工作项。
> 已落地的平台配置：Tauri 与 Gradle 最低 API 同步为 33；Activity 显式允许自由调整窗口，并接收密度、字体和布局方向变化。A1 已选择性更新 Kotlin 2.2.10 与编译 DSL；A5 本轮加入默认关闭的原生权限/状态适配器和两类前台服务，已通过 SDK 36 Kotlin 编译与 10 项 JVM 策略测试。后台实际保活、原生导入与多窗口仍需设备验收，不能以编译结果代替。

DSH 账户安全存储接入共享 Rust 凭据接口；平台固定 Android native keyring 1.0.0 的局部补丁：新命名空间显式生成 AES-256/GCM 密钥，磁盘提交失败返回错误，失败后的缓存读取隔离，缺失记录的删除重试仍确认落盘，损坏编码不当作空账号。原始许可证、来源及补丁边界见 [后端补丁说明](../../../vendor/android-native-keyring-store/AGENTERO_PATCHES.md)。云备份和设备迁移排除此密文 preferences 文件与过渡期 Bridge 凭据目录。两项后端主机测试和 Android ARM64 编译已通过；设备硬件密钥、掉电和后台生命周期仍未验收。Bridge 身份、配对与同步凭据的系统存储迁移、显式重置和失败保留已在共享层接入；真实设备密钥、升级与恢复仍需验收。

当前补齐：MainActivity 在 Tauri/Wry 真正挂载内容后，以系统栏、display cutout 与 IME 的最大边距适配实际 WebView 视口；共享 CSS 安全区域只在视口边缘宿主生效。原生 `StylusAdapterRegistry` 观察公开输入源并预留品牌适配器，标准笔输入、侧键及长按由共享层处理，不宣称通用笔身双击或物理连接检测。原生 Kotlin 插件与应用编译、JVM 策略测试通过；完整 ARM64 release APK、v3 测试签名、ABI/ZIP 与 16KB 对齐已验证，最低 API 33、target 36，设备几何、掌触和厂商手势尚待验收。共享层另修正无 `HOME` 时的私有 cache 暂存与任务/事件的规范 Vault 身份。系统返回已接入共享层的公开 SDK 回调与草稿保护，按下文继续做设备验收；原生分享与多窗口仍待实现。

## 1. 结论

- 目标是 Android 平板和折叠屏展开态；手机继续可用，只用窄档布局。
- **基线中的 Android 伴侣端尚不能直接运行。** Bridge 客户端命令没有注册，配置目录也没有适配。这两项是 M0 的第一件事（A1、A2）。
- **决策 2（申请权限后常驻后台）在 Android 上可以实现**：前台服务加常驻通知，引导用户关闭电池优化。做法参考 DSH Go 和 ds-harness-remote 的 Android 客户端（A5）。它不是绝对保证，部分厂商系统在息屏后仍会冻结应用。
- 窗口缩放（分屏、自由窗口、折叠）由共享篇的三档布局覆盖，清单只需小改。「在新窗口打开论文」要做技术验证（A6）。
- 手写先验证 WebView 路线；不达标时 Android 有现成的 Jetpack Ink 库可用，而且让手指穿透到 WebView 比 iOS 容易（A7）。

## 2. 基线定位（`66ecc126`，不代表当前实现）

| 项 | 现状 | 位置 |
|---|---|---|
| 工程 | 生成的 Android 工程在仓库里（与 iOS 不同），清单和 Gradle 直接改这里 | `src-tauri/gen/android/` |
| 版本 | minSdk 33，targetSdk / compileSdk 36；AGP 8.11.0、Kotlin 2.2.10、Gradle 8.14.3（本轮仅选择性合并 Kotlin/Java 编译 DSL） | `src-tauri/gen/android/app/build.gradle.kts:31`、`src-tauri/gen/android/build.gradle.kts` |
| 清单 | 保留联网、可选相机、`singleTask` 主 Activity、窗口配置变化与 `agentero://`；本轮由插件合入 5 项后台权限、2 个非导出服务与 Stop 接收者 | [应用清单](../../../src-tauri/gen/android/app/src/main/AndroidManifest.xml)、[插件清单](../../../src-tauri/plugins/agentero-mobile/android/src/main/AndroidManifest.xml) |
| Activity | 只开启了全面屏 | `src-tauri/gen/android/app/src/main/java/com/poco_ai/agentero/MainActivity.kt` |
| 命令 | 走 `not(ios)` 分支，注册的是 Bridge Host 命令；前端调用的客户端命令未注册 | `src-tauri/src/app/handlers.rs:173`、`:196`、`:333` |
| 目录 | 落到 `$HOME/.config`、`.cache`、`.local/share`，没有用应用私有目录 | `crates/agentero-core/src/paths.rs:27` |
| 密钥 | Bridge 身份是权限 0600 的 JSON 文件 | `src-tauri/src/integration/bridge/identity.rs:285`–`:305` |
| 备份规则 | 云备份与设备迁移均排除设备绑定密钥密文 preferences 和过渡期 Bridge 凭据目录；本轮保持不变 | [备份规则](../../../src-tauri/gen/android/app/src/main/res/xml/data_extraction_rules.xml) |
| 文件共享 | `FileProvider` 暴露了整个外部存储根目录和缓存目录 | `src-tauri/gen/android/app/src/main/res/xml/file_paths.xml` |
| 返回键 | 移动壳没有处理返回 | `src/components/mobile/` |
| CI 与发布 | PR 上检查 `aarch64-linux-android` 的 Rust 编译；发版时构建签名 APK 放进 GitHub Release 草稿 | `.github/workflows/ci.yml:82`、`.github/workflows/release.yml:253` |

CI 的编译检查发现不了命令注册错配，因为前端是按字符串调用的（共享篇 S0 会把它们接入类型绑定）。

## 3. 平台工作项

### A1 工程与配置

- **命令与状态**：按共享篇 S0 收敛。Android 当前比 iOS 多一层问题——除了命令注册错，`app/mod.rs:170` 还初始化了文件监听、连接器、Word、MCP、SSH 远程这些桌面状态。
- **按固定版本选择性合并工程模板**。本轮核对安装的 CLI 2.12.4 与 Rust Tauri 2.12.5：实际使用的 `tauri-android` 库是 SDK 36，并使用 `kotlin.compilerOptions` DSL；应用从 Kotlin 1.9.25 更新至 2.2.10，显式统一 Java/Kotlin 字节码目标为 1.8。GitHub 上 [CLI 2.12.4 的模板](https://github.com/tauri-apps/tauri/tree/tauri-cli-v2.12.4/crates/tauri-cli/templates/mobile/android)已采用 SDK 37 / AGP 9.3.1，故不直接运行初始化覆盖现有工程。保留本轮目标 SDK 36、AGP 8.11、Gradle 8.14.3，以及原 applicationId、签名、相机、MainActivity、私有目录和备份规则；不为后台适配器引入 Compose 或额外引擎。
- **最低版本定为 Android 13（API 33）**，以体验与功能优先，不承担旧系统适配。Android 13 的 `MotionEvent.FLAG_CANCELED` 可识别掌触取消，配合已有多窗口能力形成手写与布局基线。这个选择是产品支持范围，不能说 Ink 库强制要求 Android 13：已核对 Ink authoring 1.0.0 的 minSdk 为 23、graphics-core 1.0.4 为 21。实施时同步更新 Gradle minSdk、Tauri Android 配置、CI 的 NDK 编译 API（当前 `.github/workflows/ci.yml:109` 是 24）和发布检查；target/compileSdk 与最低版本分开管理。曾按客户需求短暂下探到 Android 12（v0.16.1，见第 22 节），现已撤回，后续不再支持低于 13 的系统。
- Android 14 的用户发起传输、`specialUse` 服务类型以及 Android 15+ 的超时规则按版本分支处理；Android 13 用其适用的前台服务声明和启动方式。系统版本达标也必须检查 WebView、手写笔及设备性能。
- **清单改动汇总**：

  | 改动 | 用途 | 归属 |
  |---|---|---|
  | 权限：`FOREGROUND_SERVICE`、`FOREGROUND_SERVICE_SPECIAL_USE`、`FOREGROUND_SERVICE_DATA_SYNC`、`POST_NOTIFICATIONS`、`REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` | 后台保活 | A5 |
  | 权限：`ACCESS_NETWORK_STATE` | 网络变化后立即重连 | A9 |
  | 保活服务声明（带类型和用途说明） | 后台保活 | A5 |
  | 意图过滤器：打开 PDF、接收分享的 PDF / 文本 / 图片 | 导入 | A4 |
  | `configChanges` 增加 `density`、`fontScale`、`layoutDirection` | 换屏、接显示器时不重建 Activity | A6 |
  | Activity 的最小窗口尺寸 | 自由窗口下不被拖得过小 | A6 |
  | 备份规则 | 排除密钥、缓存和可重新下载的 PDF | A2、A3 |

- **本地插件**：共享篇第 5 节的 `agentero-mobile` 插件，Android 侧用 Kotlin 实现。
- CPU 架构：发版目前只出 arm64。x86_64 只在模拟器和少数设备上需要，按需加进发版矩阵。

### A2 路径与存储

- **配置、缓存、数据目录**：`paths.rs` 已经优先读 `XDG_CONFIG_HOME`、`XDG_CACHE_HOME`、`XDG_DATA_HOME`。最小改动是在 `MainActivity.onCreate` 里、Rust 初始化之前，把这三个环境变量设成应用私有的文件目录和缓存目录。必须在原生库加载前设置，因为设置读取发生在 Tauri `setup` 之前（`src-tauri/src/app/mod.rs:137`）。如果实测时序不可靠，退回到在 Rust 里通过 JNI 取目录。
- **Vault 位置**：应用私有内部存储下的 `vault/`。其他应用看不到，卸载即删除。
- **对系统文件应用可见**（对应共享篇 D4）：实现一个文档提供器，把 Vault 目录暴露给系统「文件」应用，读写都经它。放在 M5；在那之前靠导出和分享。
- **备份**：设置备份规则。密钥密文必须排除——它们绑定在本机密钥库上，恢复到另一台设备后无法解密。缓存和可重新下载的 PDF 也排除；笔记和标注保留。
- 不申请全盘存储权限，不直接读写公共目录。

### A3 密钥存储

- 在系统密钥库里生成一把不可导出的 AES 密钥，用它加密凭据后存到应用私有目录。迁入：Bridge 身份与配对资料、同步凭据、远程 Agent 的设备私钥与令牌、主机公钥固定表。
- 已有的 JSON 身份文件首次启动时迁移并删除。
- 密钥不要求用户每次解锁验证，否则后台同步读不到。

### A4 导入、分享、导出

| 入口 | 做法 |
|---|---|
| 应用内选文件 | 已接入 Rust 所有的系统 PDF 选择与私有 cache 流式暂存，再复用同一入库内核；不会把整份文件读入 WebView。固定版本 fs 插件丢失 asset 子区间信息，因此 只接受 `content://`，拒绝外部结果中的普通路径或 `file://` 回退。Rust 私有适配器在打开/查名称前验证精确 provider，拒绝未知 provider、应用相同 UID 和带用户前缀的跨资料 URI，再调用 `openFileDescriptor("r")` 获取完整文件/pipe，拒绝子区间和空 descriptor；可选 `DISPLAY_NAME` 仅作受限文件名。提供器打开/查询在有界后台线程执行，临时授权在入库前复制完毕，不保存 URI、不扩张 renderer 权限。详见[论文入库](pdf-open.md)；云提供器与真实设备仍待验收 |
| 「打开方式」 | 已加入仅 `ACTION_VIEW` / `application/pdf` / `content` 的过滤器，采用固定 SDK 公开的 `Plugin.onNewIntent` 与插件 `load` 一次性读取启动 Intent，不依赖桌面 `RunEvent::Opened`。只接受具备当前读取授权的单份 URI；相同 URI 的单项 ClipData 可作授权运输，文字、多份和额外 stream 拒绝。原生单槽邮箱保留第一份，忙碌给本地化提示；冷暖启动不重复领取，不持久化授权、不把 URI 交给前端。当前本地 Vault 就绪后与选择器共享准入锁，复用 provider UID 校验、完整 FD、私有流式暂存与原有入库内核，内容仍须通过 PDF 标头和字节上限。仅刷新文献库，不强制切走草稿/当前论文；真实文件管理器、云提供器及冷暖启动授权仍待设备验收。详见[系统打开 PDF](pdf-open.md) |
| 系统分享 | 已接入单份 PDF 的 `ACTION_SEND`：独立 DEFAULT/PDF MIME 过滤器不加 scheme，因为 URI 位于 `EXTRA_STREAM`。使用固定 Core 1.17.0 的 `IntentCompat` 兼容 API 33 类型化 Parcelable 缺陷；必须成功解码单个 Uri，data 只能为空或与其相同，ClipData 只允许同一 URI 的单项，其他文字/HTML/嵌套 Intent、多份和冲突来源拒绝。复用打开方式的读取授权、content-only/provider UID、原生单槽邮箱、当前库准入与流式入库，不把 URI 交给 JS、不持久化授权、不强制切走当前文档。插件/应用 Kotlin 检查与 61 项 JVM 检查、41 项前端交接/工作区回归通过；真实 Sharesheet、Parcelable/授权与提供器行为仍待设备验收。多份/文本分享、iOS 分享与长期收件箱队列仍待实现。详见[系统打开与分享 PDF](pdf-open.md) |
| 导出 | 「另存为」走系统的创建文档界面；分享走 `FileProvider` |
| 扫描纸质文档 | 后置。常用的文档扫描组件依赖 Google 服务，国内设备上不可用 |

已收紧 FileProvider 的路径映射：移除整个外部存储与整个 cache，只预留 `cache/exports/` 给显式出站副本，并保留固定 Wry 相机捕获所需的应用专属 `external-files/Pictures/`。Provider 仍非导出且仅用临时 URI 授权；映射存在不等于公开读取，不声称曾发生泄漏。当前批注 PDF 另存为使用 SAF，不依赖 FileProvider；未来出站分享需真正的副本写入/授权/清理实现，本轮没有新增该功能。详见[FileProvider 目录边界](pdf-open.md#出站-fileprovider-的目录边界)。

### A5 后台保活与通知

**仅 Android 提供常驻申请**。设置里新增「后台运行」页，由用户主动开启，展示通知、电池优化和厂商限制的实际状态；拒绝授权后仍可前台使用。iOS 不出现这个入口。

已从 DSH Go 部分克隆按需取回并阅读 `DshService.kt`、`PetOverlayService.kt`、`background_guide_page.dart`、`background_net.dart`（固定版本 `74e1faa`）。可借鉴的是前台通知、退出释放唤醒锁、网络受限检测、回到设置后刷新状态，以及厂商设置页不可用时退回应用详情页。原服务面向本机引擎或悬浮宠物，其用途说明不适用于 Agentero；本方案不引入本机 Agent、悬浮窗、root、Shizuku 或无障碍权限，也不复制 AGPL 实现。

设置页提供以下引导：

1. 允许通知（Android 13 起要运行时申请）。
2. 关闭电池优化（跳到系统的豁免确认框）。
3. 厂商系统的自启动和后台限制、在最近任务里锁定应用——这些没有统一接口，只能给出分品牌的操作说明。

**机制**：

| 场景 | 做法 | 说明 |
|---|---|---|
| 用户开启后台运行且远程 Agent 会话进行中 | Android 14+ 拟用 `specialUse` 前台服务，声明真实的远程会话用途；常驻通知含「停止」按钮；必要时持有部分唤醒锁并在结束时释放 | 类型选择需与实际用途、分发规则验证；不能照抄 DSH Go 的本机引擎/宠物用途，也不能把 `remoteMessaging` 当通用保活类型 |
| 同步、批量下载进行中 | 独立的 `dataSync` 前台服务，与 Agent 保活分开计数和停止 | Android 15+ 的相关目标版本受每 24 小时累计 6 小时限制；超时必须落盘并停止，不假定任务一定能在配额内完成 |
| 应用不在运行时的定期同步 | 系统的延迟任务调度，最短间隔 15 分钟，具体时机由系统决定 | 任务被唤起时没有界面，需要通过 JNI 直接调用同步入口（共享篇 S3 第 1 点）。与正在运行的应用靠 Vault 同步锁互斥。可行性在 M2 开头验证；不行就只在应用存活时同步 |
| 用户发起的大批量下载（Android 14 起） | 可选：用户发起的数据传输任务 | 系统为它显示通知，受限更少。作为前台服务之外的备选 |

**服务生命周期**：

- 由共享篇的保活接口驱动：持有的令牌数从 0 变 1 时启动，回到 0 时停止。
- 只在应用可见时启动。Android 12 起从后台启动前台服务受限；用户已关闭电池优化是豁免条件之一，但不依赖它。
- 用户把应用从最近任务划掉，服务随之停止。远端主机上的任务不受影响，下次打开后重连取结果。
- 连接本身在 Rust 里，前台服务的作用是让进程保持前台优先级。WebView 的脚本在后台可能被节流，不影响连接。
- 每 25 秒做一次带超时的探活（与上游一致），断线后有限次数重连；重连用尽就停止服务并发一条通知。
- 通知种类按共享篇 S8，各自独立的通知渠道，用户可以单独关闭。锁屏只显示泛化文案。

**做不到的**，要写进用户文档：

- 部分厂商系统在息屏后会冻结应用并切断它的连接，即使前台服务和唤醒锁都在。ds-harness-remote 的开发记录里有小米设备息屏约 6 秒后被冻结的实测。所以保活是尽力而为，远端任务本身不会丢，亮屏后自动重连。
- 如果以后上架 Google Play：`specialUse` 类型要在上架时说明用途并接受审核；直接弹出电池优化豁免请求只允许用于核心功能确实受影响的应用。目前是侧载 APK，不受这两条约束（D-And-3）。

**技术验证（M0）**：主 Activity 被销毁而服务还在时，Rust 运行时和连接是否存活；重新进入时会不会重复初始化。结论决定「划掉即停止」是策略选择还是硬性限制。

#### 本轮原生适配器边界

`agentero-mobile` 的 Android 适配器使用系统 Service、通知渠道和 Tauri 公开权限回调。对比了 [通用 background-tasks 插件](https://github.com/guillemcordoba/tauri-plugin-background-tasks)的 WorkManager 定期执行方案，以及 [Holochain foreground-service 插件](https://github.com/mattyg/tauri-plugin-holochain-foreground-service)的本机引擎用途；它们不提供本轮所需的双服务租约、实际启动确认和异步代次隔离，故不新增依赖或复制其代码。DSH 远程服务只参考生命周期设计，不采用其 `remoteMessaging` 类型、无界唤醒锁或带任务内容的锁屏通知。

- 默认关闭，仅持久化独立 preferences 中的 `enabled` 布尔值。通知权限由 Android 与 Tauri 的标准权限状态负责，不保存任务名称、提示词、Host 信息或凭据。
- 显式开启要求应用可见、`POST_NOTIFICATIONS` 获准且两个后台渠道可用。查询状态不请求权限；申请通知和打开电池/通知/应用设置只能由前台显式操作触发，设置不可用时退回应用详情。
- 可见性按已 started 的 Activity 集合计算，不依赖窗口焦点或 resumed 状态。分屏中一个 Activity 停止不会把另一个仍可见的窗口算作后台；集合和事件接收者不强持有 Activity。
- Rust 事件中继与 Activity 可见性分开管理。已核对 SDK 2.12.5：`PluginManager.onDestroy` 移除 Activity 的结果启动器，但保留进程级 PluginHandle、插件监听器和 Rust channel。因此 Activity 销毁时不能注销唯一的弱事件中继，否则仍存活的 dataSync 服务超时后无法通知 Rust 做检查点。Application 的 `onActivityDestroyed` 仍独立清理可见性弱集合；同一插件的 load/resume 由弱集合去重，新插件替换后的旧中继随引用释放自然清理。这里只保留对插件的弱引用，不增加 Activity 强持有；实际无界面超时仍需设备验收。
- Agent 使用独立 `specialUse` 服务（API 34+）；API 33 使用正常前台服务调用。同步和传输合用独立 `dataSync` 服务，不与 Agent 的计数混合。`agentActive` / `syncActive` 只在系统 `startForeground` 成功后置为真，排队启动不算活动；启动确认最多等待 4 秒，失败向 Rust 返回错误。
- Rust 发送无任务内容的聚合计数及规范十进制字符串 `generation`（u64）。原生只接受更大的代次；迟到或重复的 apply 返回实际状态，不覆盖新计数。后台可更新已经运行的服务，不能新启动；计数归零、关闭设置或失去权限则停止对应服务。
- 两个服务均 `START_NOT_STICKY`、`stopWithTask=true`，无开机接收者、自动重启、WakeLock、悬浮层或本机引擎。清理不依赖 `onDestroy` 一定被调用：没有需要跨进程释放的锁，进程结束也不会自动复活服务。划掉任务会停止载体，保留用户偏好；通知中的「停止后台活动」额外关闭偏好并停止两种服务，不取消远端 Host 任务。
- Android 15+ `dataSync.onTimeout` 先停止服务，再发 `background-expired` 的 `{kind:"sync",reason:"timeout"}`，供 Rust 中止/检查点本地同步及传输；Agent 不受该超时影响。原生在 Rust 清空这两类计数前不重启超时载体。
- 通知的正文、渠道和按钮使用 Android en / zh-CN 资源，仅显示泛化文案，锁屏 private/public 版本均无论文、提示词或 Host 内容。各渠道独立检查；关闭同步渠道不应停止仍获准的 Agent 载体。

验证：使用 JDK 17.0.21、Gradle 8.14.3、Kotlin 2.2.10、SDK/Build Tools 36，运行 `:tauri-plugin-agentero-mobile:compileDebugKotlin` 与 `:tauri-plugin-agentero-mobile:testDebugUnitTest`。10 项 JVM 用例全部通过，覆盖多窗口 started 计数、默认关闭、显式授权门控、后台只更新已启动载体、排队启动去重/取消、聚合租约归零、u32 边界、通知撤销、同步超时隔离及 u64 代次晚到/重复/非法输入。任务图仅包含本插件与 `tauri-android`，没有执行应用 Rust/Cargo 构建；依赖项目构建产物重定向到 Android 工程的忽略目录，未修改共享 SDK 源码。上游 SDK 的旧生命周期方法/Kotlin DSL 警告及受限环境中的 analytics 写入警告不影响通过结果。

另外通过 `:tauri-plugin-agentero-mobile:assembleDebug` 和 `:app:processArm64DebugMainManifest`，仅生成 Android AAR 与合并清单，没有构建 APK。AAR 中包含插件、双服务、Stop 接收者和控制器；合并清单确认 `specialUse` / `dataSync` 类型、非导出、`stopWithTask=true`、5 项必要权限与真实 specialUse 用途声明，未引入 WakeLock 或开机权限。应用 ID `com.poco_ai.agentero`、API 33/36、可选相机、`singleTask`、可缩放窗口和备份规则仍保留；签名配置与 MainActivity 未改变。

该层不实现后台定期调度、UIDT 下载、网络切换重连或完成通知。未连接真实 Host，尚不能据编译/JVM 测试宣称息屏、划任务、系统超时、厂商保活或 Activity 销毁后的 Rust 生命周期已通过设备验收。

### A6 分屏与多窗口

**窗口缩放**（必须支持）：

- targetSdk 已是 36。Android 16 在最小宽度不小于 600dp 的屏幕上会忽略应用对方向、宽高比和可缩放性的限制，所以应用一定会被放进分屏和自由窗口。
- 清单已声明自行处理屏幕尺寸变化，缩放时 Activity 不会重建，WebView 状态保留。补上 `density` 等几项，避免拖到另一块屏幕时重建。
- 分屏时本应用可能可见但不是当前焦点窗口。视频、相机这类独占资源我们没有，主要影响是键盘快捷键只在获得焦点时生效。
- 折叠屏展开和合上会让宽度跨档，由共享篇 S4 的三档布局处理。
- 设置最小窗口尺寸。

**多窗口**（「在新窗口打开论文」）：

- 固定 SDK 的核对修正了早期假设：Tauri 2.12.5 公开 `activity_name`，但默认 builder 不保证新 Activity。Tao 0.37.2 可能复用可用的主 Activity，Wry 0.57.1 随后替换它的 WebView；直接选择当前 `singleTask` 的 MainActivity 也会复用实例。Android `set_focus` 在此 SDK 中尚未实现，标签/返回成功不能作为独立窗口或聚焦确认。当前不开放新窗口入口；按窗口路由持久化先由共享层接通，文档 Activity、稳定路由映射和原子预留仍待实现。目标有两种形态：
  - 独立任务窗口：新 Activity 作为单独的任务启动，并请求放到相邻窗口。在分屏和自由窗口模式下，两篇论文各占一个可独立缩放的系统窗口，最近任务里各有一项。**这是目标形态。**
  - Activity 嵌入：两个 Activity 并排在同一个任务窗口里，需要 Android 12L 以上的大屏设备和 Jetpack WindowManager 的分栏规则。应用内的并排已经由 S4 的布局提供，这种形态增益不大，不做。
- 主 Activity 保持 `singleTask`（链接和分享要回到同一个实例），文档窗口用另一个 Activity 类。
- 每个窗口一个 WebView，各自加载一份 PDF 渲染引擎。先量内存再定窗口上限。
- 系统会在后台销毁 Activity。按窗口标签落盘路由，重建时恢复（共享篇 S7）。
- **M0 技术验证的出口条件**：在平板上打开第二个窗口并与主窗口并排；两个窗口各自调用命令、各自收到事件；旋转、缩放、切到后台再回来后状态不丢。tao 的构建方法不够用时，在本地插件里用 Kotlin 启动文档 Activity。验证不通过则多窗口后置，窗口缩放不受影响。

### A7 手写笔

数据格式、存储、同步、显示都在共享篇 S6。这里只定输入层。

**先验证路线 A（WebView 指针事件）**，M0 在至少两台不同厂商的平板上实测：

- 笔尖到墨迹的延迟；
- 压力、倾角、合并事件、预测事件的可用性和采样率；
- 掌触：系统判定为手掌的触摸会被取消。要确认 WebView 是否把它转成指针取消事件；能收到就在收到时撤销这一笔，收不到则路线 A 不成立。

**路线 A 不达标时用路线 B**：在 WebView 上盖一个 Jetpack Ink 的书写视图。Ink 库 1.0.0 已是稳定版，提供低延迟的落笔渲染。Activity 是视图体系，用它的 View 版本，不引入 Compose。

- 书写层在触摸按下时按工具类型分流：手写笔和橡皮端由它处理，手指触摸不消费，落到下面的 WebView，滚动和缩放照常。
- 落笔结束后把输入点（坐标、时间、压力、倾角、方位）交给前端，换算到 PDF 页面坐标并按共享格式保存，然后清空原生层。不用 Ink 库自带的存储格式。
- 前端把当前页面的缩放和偏移同步给书写层，保证落笔位置和页面对齐。

两条路线共有：

- 笔的橡皮端和侧键映射为橡皮和套索；没有这些硬件时用工具栏。
- 悬停只作增强。
- 厂商专属能力（例如 S Pen 的隔空手势）不做。
- 系统的「记事」角色和锁屏快速记事入口不做。

### A8 WebView 与输入

- **WebView 版本**：已接入 React 加载前的原生保护页。保留完整 Tauri/Wry 生命周期，在首次 URL 导航前用公开 `WebSettings.getDefaultUserAgent` 检查实际 Chromium 内核；已知低于 111 才关闭该 WebView 的 JavaScript，并通过两个 `setContentView` 重载替换为不依赖 HTML/CSS 的英中原生更新提示，继续复用系统栏/键盘安全区域。111 及以上和 unknown 均保留 SDK 视图，不把厂商 provider 的自定义应用版本号误当内核版本；unknown 不宣称已验证兼容。设置仅在用户点击后打开当前 provider 的应用详情并允许系统设置回退，不扫描浏览器、不自动下载或换提供器；关闭仅 `Activity.finish`。Activity 收到配置变化时先执行 Tauri/AppCompat 回调，再只重建已显示的原生提示页；重新解析当前语言、SP/DP 与 DayNight 主题，保留父容器、位置、布局参数及安全区域，不重建 Activity/WebView、不改 JavaScript 或重载论文。9 项内核策略与既有用例共 47 项 JVM 检查通过，配置刷新后的插件与 MainActivity Kotlin 均重新编译通过；这些检查不渲染原生页面，实际字体缩放、语言切换、深色模式、无障碍、旋转与更新回跳仍待设备验收。详见[WebView 兼容性](webview.md)。
- **返回手势**：已接入固定 SDK 的 `app.onBackButtonPress`，只在拥有弹层、阅读/Agent/广场状态或未保存修改时注册单个回调。成熟 Radix 层先按可取消 Escape 路径关闭当前顶层，忙碌确认不会穿透；再关闭侧栏/历史、折叠研讨、返回文献库或检查笔记/笔迹退出保护。干净文献库根页面提前注销回调，恢复 SDK 的 WebView/系统分派，不调用 `app.exit`（它会 `finishAffinity`），不申请退出权限、不停止前台服务。注册/移除串行并隔离迟到事件；SDK 的异步交接窗口仍需设备检查。确认 Android 平台后停用自定义左右滑动与阅读边缘返回，不取消普通滚动/笔输入。嵌套扫码页改用现有 Dialog，关闭后迟到的相机控制器也停止。真机预测返回、输入法优先级与任务根行为仍待验收。
- **软键盘**：`MainActivity` 开了全面屏，键盘弹出时窗口不会自动让位。要么把键盘高度传给前端，要么确认可视视口接口在这种模式下给出正确的值——M1 开头验证。
- **外接键盘和鼠标**：快捷键走 Web 键盘事件；有鼠标或触控板时保留悬停菜单和右键菜单。
- **进程被杀后的恢复**：按落盘的路由恢复到原论文原位置；草稿靠自动保存（共享篇 S2）。

### A9 远程 Agent 的 Android 部分

- 默认使用 ds-harness-remote，通过 `mobile` 的 provider 契约接入。前台服务只感知统一连接和任务状态，不绑定 DSH 的消息结构；新增 provider 无需重写保活与通知。能力不足时返回明确的不支持状态。
- 已实现 Application 所有的网络观察：默认网络与应用可访问的非默认 LAN/VPN 使用两个被动回调，普通 `ACCESS_NETWORK_STATE` 权限无运行时弹窗。回调仅向 Rust 发送单调计数字符串，旧网络丢失、重复能力与链路快照按策略去重，不输出地址或 SSID，也不以公网验证阻止局域网。Activity 销毁与关闭后台设置不撤销进程观察；恢复只提示已有传输 owner，显式断开/未知操作不重放。原生 Kotlin 编译及 20 项 JVM 策略测试（其中 10 项新增网络用例）通过；真实 Wi-Fi/蜂窝/VPN/LAN 切换与后台恢复仍需设备验证。共享契约与完整恢复边界见[移动网络恢复](connectivity.md)。
- 账号登录在系统浏览器里完成。上游的浏览器登录流程靠轮询取结果，不需要回跳链接；需要时可用已注册的 `agentero://`。
- 发版构建禁用了明文流量，这只约束 WebView 和 Java 网络栈。远程 Agent 的连接由 Rust 发起，共享篇已规定明文 HTTP 只允许回环和私网地址。
- 扫码配对复用现有的相机权限。

### A10 分发

- 沿用现状：签名 APK 随版本放进 GitHub Release。没有应用内更新（更新插件只在桌面注册），在设置里显示当前版本和下载页入口。
- 新增的权限会在安装和首次使用时出现，发布说明里逐项解释用途。
- 用同一把签名密钥，保证覆盖安装。

## 4. 里程碑中的 Android 部分

| 里程碑 | Android 工作 |
|---|---|
| M0 | A1（命令注册、模板更新、插件骨架）；A2 的目录修复；A3；三项技术验证：A5 的 Activity 销毁后运行时存活、A6 多窗口、A7 手写路线 A |
| M1 | A2 的 Vault 与备份规则；A4 的选文件与「打开方式」；A8 |
| M2 | A5 的同步前台服务与定期同步 |
| M3 | A5 的 Agent 保活、通知、「后台运行」设置页；A9 |
| M4 | A7 定型（路线 A 或 B）、橡皮端与侧键 |
| M5 | A6 多窗口落地；A4 的系统分享；A2 的文档提供器；A10 |

## 5. 真机验收

设备与系统：

- 一台原生系统平板（Android 16）、一台三星平板（S Pen、DeX）、一台小米或其他国内厂商平板（后台限制严格，可能没有 Google 服务）、一台折叠屏、一台手机（回归）。
- 覆盖 Android 13（最低版本）、14（服务类型与数据传输）、15（超时限制）、16（大屏窗口规则），发布时再覆盖当时最新稳定系统；低于 API 33 的设备不进入支持矩阵。

清单：

1. 首次启动进入本地 Vault；Bridge 配对、恢复、数据请求都能完成。
2. 飞行模式下导入、阅读、高亮、写笔记；杀进程后内容都在。
3. 分屏、自由窗口、折叠与展开、旋转后，页码、缩放、草稿不变，Activity 没有重建。
4. 两个窗口并排读两篇论文；后台被回收后恢复到原位置。
5. 发出 Agent 任务后息屏 1、5、15 分钟：记录连接是否保持；断开的情况下亮屏后自动重连并补齐结果。分别在默认省电和关闭电池优化两种设置下测。
6. 划掉应用后保活通知消失，唤醒锁释放；再次打开能取回远端结果。
7. 同步进行中切到后台，同步完成或从断点继续；Wi-Fi 与蜂窝切换不产生重复上传。
8. 手写：缩放和旋转后笔迹不偏；手掌搁屏不误触也不留下笔迹；橡皮端可用。
9. 从浏览器和文件管理器分享 PDF、链接到 Agentero，回到应用后已入库。
10. 返回手势逐级返回，不会从阅读页直接退出。
11. WebView 版本过旧的设备上出现提示而不是错乱界面。

## 6. 待决事项

| # | 事项 | 建议 |
|---|---|---|
| D-And-1（已定） | 体验与功能优先的最低版本 | Android 13 / API 33，实施时同步构建配置和 CI；Android 12 支持曾短暂开放（v0.16.1）后撤回，见第 22 节 |
| D-And-2 | Vault 是否通过文档提供器对系统文件应用可见 | 可见，放在 M5 |
| D-And-3 | 是否上架 Google Play | 暂不上架。上架会让 `specialUse` 前台服务和电池优化豁免请求进入审核范围 |
| D-And-4 | 划掉应用后是否继续保活 | 不继续。行为可预期，远端任务不受影响 |
| D-And-5 | 是否支持 HarmonyOS NEXT 等不兼容 Android 应用的系统 | 不在范围内 |

## 7. API 阅读与笔迹保存验证（2026-11-14）

已合入共享移动阅读改动：宽档论文列表折叠、应用内 API 翻译/Paddle/MinerU 设置、显式排版与翻译任务、完成块/逐页 IR/原生外部任务回执恢复。Android actor identity 保存只 fsync 自身 ink 与应用配置目录，不再打开 `/data/user/0`；失败仍保留待保存笔迹。移动 renderer 仅获得私有 Vault/cache 文件权限，外部任务回执及其祖先操作由原生作用域拒绝。

共享宿主全量 951 通过、10 项既有环境忽略；全量前端 3573 通过、10 项既有跳过；类型、依赖、绑定/事件契约及严格 Clippy 通过。原生 macOS 移动预览完成启动与私有库创建，控制服务不能激活/捕获其窗口，故不计为可见 PDF/笔硬件验收。API 恢复测试使用本地 HTTP fixture，未调用真实付费服务。


## 8. 论文滑动操作、设置页与广场工具栏验证（2026-11-14）

已合入共享移动 UI 改动（mobile `260d128af`，Android merge `2420f11c3`）：

- 文献库论文行支持左滑露出操作层，本阶段只提供删除；删除经原生 `path_delete` 永久删除论文目录与 catalog 行（移动端没有回收站恢复界面，不写 `.agentero/.trash/`），二次确认后刷新 Catalog，当前论文草稿未保存时拒绝删除。
- 设置页按电脑端结构聚合外观、翻译、排版解析、同步、后台运行与关于；侧栏只保留一个设置入口，旧的同步弹窗、处理设置弹窗和后台 Dialog 已移除。翻译/排版仍只走远程 API。
- 广场标题、来源下拉、刷新图标与图标式打开浏览器合并到同一 header 行；广场内魔棒与内嵌网页的第二层工具条已删除。

共享宿主本轮无 Rust 改动；全量前端 463 文件、3587 项通过、10 项既有跳过；TypeScript、依赖图（1482 模块、8161 条边）及 scoped BIOME 通过。新增覆盖包括真实 DOM 的论文行滑动/确认删除/草稿拒删、创作设置页分区切换与 apiOnly、真实 App 中设置页接入及 reader 不重挂载、广场控件与 header 重挂载，以及安全区宿主。真实设备触摸滑动、删除确认、回收站恢复、设置页滚动与广场 header 视觉仍未验收。


## 9. 手写笔适配重新打包验证（2026-11-15）

Android 分支合入 OPPO/OnePlus 笔身快捷动作与笔悬停保护（`c28d52f03`、`21be5b815`、`b321357ac`）：

- `PencilShortcuts.kt` 只接收厂商公开的 `com.oplus.ipemanager.action.PENCIL_DOUBLE_CLICK` 广播，按 `ipe_pencil_double_click` 系统设置映射橡皮/上一个工具/调色盘；没有引用 OEM SDK，也没有新增 IPC 或绑定。注册失败时能力报告为空，首次真正收到事件后才标记可用。
- 共享 PDF 笔输入增加 1 秒悬停保护：书写模式下笔悬停时新落下的手指不启动平移、长按选择或笔迹，直到抬起；已开始的笔迹不受影响，阅读模式不冻结手指导航。

验证与产物：

- 前端 `pdf-stylus-input` 15 项与 `mobile-stylus` 5 项聚焦回归通过；Android 完整 ARM64 release 构建（含 Kotlin、Rust、资源与清单合并）成功；插件 JVM 策略测试 61 项通过（5 个测试类，0 失败）。
- 包名 `com.poco_ai.agentero`，版本 0.15.5/15005，API 33/36，仅 ARM64；合并清单新增 `com.oplus.ipemanager.permission.receiver.DOUBLE_CLICK`，FileProvider 仍仅开放相机 Pictures 与导出 cache，PDF VIEW/SEND MIME-only 且无 SEND_MULTIPLE。dex 已确认包含笔身快捷适配器字符串。
- 保留此前移动 UI、API reader 与 native Inbox 安装包。OPPO Pad mini + Pencil 3 Pro 真机事件、工具切换、悬停保护与后台拒绝仍待用户验收；笔端震动/材质触感/轻捏未实现。

## 10. 移动端永久删除重新打包验证（2026-11-15）

移动端没有回收站恢复界面，论文行左滑删除改为永久删除（commit `29de07690`）：

- 新增 `path_delete`，复用 core `delete_paths(..., permanent: true)`：直接删除论文目录及其下文件，不生成 `.agentero/.trash/` 副本，`papers/` 下 catalog 行随文件一起删除；拒绝 `papers` 根、`.agentero`、`..` 与越界路径，符号链接目录只删链接本身。
- 删除后取消被删路径（含嵌套论文）的 JobCenter 任务，并在移动端显式通知同步调度器拉取一次快照；远端 Vault 不支持该命令。
- 移动端命令白名单加入 `path_delete`，同时移除桌面回收站的 `path_trash` / `path_list_trash` / `path_restore_item` / `path_purge_item` / `path_purge_trash`。删除确认文案改为“永久删除…此操作无法撤销”；草稿未保存的当前论文仍拒绝删除。

验证与产物：

- TypeScript 通过，依赖图 1482 模块 / 8161 条边无违规；移动删除、滑动、工作区与 API 测试失败提示共 4 个文件 32 项通过。
- Android 完整 ARM64 release 构建成功；包内 Rust library 含 `path_delete` 注册字符串，而 `path_trash` 等桌面回收站命令字符串未进入移动库，确认移动白名单只暴露永久删除。
- v3 测试签名、ZIP 完整性、ZIP/native 16KB 对齐通过；包名 `com.poco_ai.agentero`，版本 0.15.5/15005，API 33/36，仅 ARM64；FileProvider 仍只开放相机 Pictures 与导出 cache，PDF VIEW/SEND MIME-only 且无 SEND_MULTIPLE。
- 保留此前手写笔、移动 UI、API reader 与 native Inbox 安装包。真机左滑删除、确认框、删除后列表刷新与文件消失仍待用户设备验收；删除不可恢复，操作前需确认。

## 11. 阅读器与翻译修复重新打包验证（2026-11-15）

本轮修复（commit `981f7a089`）：

- **参考文献解析**：移动端命令白名单补入 `paper_refs_parse`（`paper_refs_list` 原已注册）。左侧参考文献面板点击“解析参考文献”不再报 `Command paper_refs_parse not found`；解析在 Host 侧完成本地 `.bib`/`.bbl` 解析与在线结构化引用查询，不使用本地 PDF 引擎。
- **适配屏幕宽度**：移动阅读器向 `PdfViewer` 传 `reserveCommentRail={false}`。移动端没有桌面批注卡片侧栏，Fit width 不再预留 `COMMENT_RAIL_WIDTH_PX`，页面铺满实际阅读区域，旋转/分屏后也按整个 pane 计算。
- **翻译反馈与后台运行**：移动端即使共享设置仍为双栏模式，也强制在当前 PDF 上以叠加层运行全文翻译，不再尝试打开桌面翻译标签页；工具栏翻译按钮在平板上放大到 44px 触控目标。新增应用级后台任务浮条：显示翻译/排版/导入任务的标题、阶段、百分比与取消按钮，失败保留具体原因；任务由 TaskRuntime/JobCenter 执行，离开阅读页或切回文献库后继续，浮条仍在。
- **API 测试失败原因**：保留 `a0ca4219e` 的 Host 原因 toast，API 已可访问时不影响正常请求。

验证与产物：

- TypeScript 通过，依赖图 1483 模块 / 8169 条边无违规；移动壳、PDF 翻译生命周期、能力门、后台任务浮条与 API 提示共 8 个文件 121 项通过。
- Android ARM64 release 构建成功；包内 Rust library 含 `paper_refs_parse`、`paper_refs_list`、`path_delete` 注册字符串；FileProvider 仍只开放相机 Pictures 与导出 cache。
- v3 测试签名、ZIP 完整性、ZIP/native 16KB 对齐通过；包名 `com.poco_ai.agentero`，版本 0.15.5/15005，API 33/36，仅 ARM64。
- 保留此前永久删除、手写笔、移动 UI 与 API reader 包。真机需继续验收：参考文献解析在线查询、翻译点击后的浮条进度/取消、离开阅读页后任务继续、适配宽度与旋转/分屏、错误 toast 文案。

## 12. 双栏翻译、手写编辑与长按重新打包验证（2026-11-15）

本轮修复（commit `92f33f756`）：

- **按设置选择翻译呈现方式**：移动端不再强制覆盖。翻译显示模式为双栏对照时，点击工具栏翻译按钮会在阅读器右侧打开只读译文 pane，左侧原 PDF 保持完整手写/批注能力；只有设置为“覆盖在原 PDF 上”时，译文才叠在原文上，此时原 PDF 暂时只读。
- **覆盖模式的提示**：覆盖翻译显示/运行期间隐藏手写工具条（不再只剩一个下载/导出按钮），右上角翻译阶段胶囊、底部后台任务浮条显示处理进度与取消，提示当前原文不可编辑；点击翻译按钮隐藏覆盖层后手写工具恢复。
- **双栏 pane**：新译文 pane 复用 `PdfTranslationViewerInner`，自行读取/生成 `layout-translate.json` 并订阅共享翻译任务；窄屏时译文 pane 占满阅读区，关闭后回到原文。新增左上角关闭按钮；新出现的 pane 不会被组件的临时折叠状态隐藏。
- **长按调整**：工具栏翻译按钮只响应短按，不再识别长按；触控/笔长按选择 500 ms → **1000 ms**；笔悬停防误触 1000 ms → **1500 ms**。滑动超过 8 CSS px 仍取消长按。
- 上一轮的参考文献解析、适配宽度和 API 测试失败原因提示保持。

验证与产物：

- TypeScript 通过，依赖图 1483 模块 / 8169 条边无违规；手写输入/笔工具、移动壳与译文 pane、PDF 能力门、翻译生命周期、后台任务浮条共 6 个文件 117 项通过。
- Android ARM64 release 构建成功；签名、ZIP 完整性与 ZIP/native 16KB 对齐通过。
- 包名 `com.poco_ai.agentero`，版本 0.15.5/15005，API 33/36，仅 ARM64；FileProvider 与 PDF 分享边界未变。
- 真机重点验收：双栏设置下翻译是否在右侧打开且左侧可手写；覆盖设置下原文是否只读并显示进度；窄屏译文 pane 与关闭回到原文；滑动翻页不再误触发选区；翻译按钮只响应短按。
- 此前的手写长按包 `Agentero-0.15.5-pen-longpress-arm64-test.apk` 为中间版本，行为已被本包取代。

## 13. 设置、备注手势、图片查看与悬停反馈重新打包验证（2026-11-15）

本轮改动（commit `5427bf3eb`）：

- **EasyScholar 密钥**：从移动论文信息页移除，新增移动设置 → EasyScholar 分区维护密钥、探测与打开密钥页；论文标签/指标继续读取同一设置。
- **打开论文的空对照区**：恢复上次阅读路线时曾保留 `readerMode=notes`，同时预设被重置为阅读；窄屏于是显示 notes 半屏但“对照”按钮未选中。现在恢复统一进入阅读预设 + PDF 模式；窄屏 notes 仅在对照预设下显示，消除了空白右栏与按钮状态不一致。
- **备注手势**：1 秒静止长按仍用于选择文字/笔迹；继续静止到 **3 秒**添加备注：
  - 文字选区上打开选区备注编辑器；
  - 图表/表格/算法/公式区域上创建对应视觉批注；
  - 空白处提示需要选区或图表。
- **图表点击备注移除**：图/表/算法/公式命中框点击改为打开全屏裁剪查看（背景、关闭按钮或 ESC 退出），不再创建备注。hover 命中框时按 bbox 渲染裁剪浮层，约 1.03× 放大并加阴影，突出当前图片；框选/裁剪进行中不挂载。
- **全局按钮 hover**：对没有自身 hover 色的按钮补半透明浅灰底（`--muted` 55%），仅 `(hover: hover)` 指针生效，已有变体 hover 色优先。
- 上一轮的双栏翻译、手写工具、长按 1000ms / 笔悬停 1500ms 等保持。

验证与产物：

- TypeScript 通过；依赖图 1483 模块 / 8172 条边无违规；移动工作区/设置、手写输入、PDF 能力门、译文生命周期与后台任务条共 9 个文件 135 项通过。
- Android ARM64 release 构建成功；v3 签名、ZIP 完整性与 ZIP/native 16KB 对齐通过。
- 包名 `com.poco_ai.agentero`，版本 0.15.5/15005，API 33/36，仅 ARM64；FileProvider 与 PDF 分享边界未变。
- 真机重点验收：设置里能保存/探测 EasyScholar 密钥且论文页不再出现；打开/恢复论文不再出现空右栏或错误预设按钮；1 秒选择、3 秒备注、图表 hover 放大与点击全屏；按钮 hover 浅灰反馈。

## 14. WebView 缩放与滑动选择修复重新打包验证（2026-11-15）

本轮修复（commit `a0445ee39`）：

- **全屏图片后整个应用被放大**：Android WebView 允许浏览器级页面缩放，查看全屏图片时双指/双击缩放会把壳层 UI 整体放大，且网页层无法复位。现在 `MainActivity.onWebViewCreate` 关闭 `supportZoom` / `builtInZoomControls` / `displayZoomControls`，并把页面 viewport 收紧为 `maximum-scale=1, user-scalable=no`；PDF 缩放与图片查看仍由应用内视图控制，不再影响全局界面。
- **文字上直接滑动触发选中、翻页卡顿**：单指在 PDF 上滑动时，`bindInkInput` 现在会阻止单指 `touchmove` 的默认行为，避免 WebView 原生文字选择与自定义 pan 抢手势；双指缩放仍放行给 ZoomGestureWrapper。1 秒长按选择、3 秒长按备注不变。
- 上一轮的 EasyScholar 设置、备注手势、图表 hover/全屏、双栏翻译等保持。

验证与产物：

- TypeScript 通过；Biome 无告警；手写输入 18 项（含单指 touchmove 被阻止、双指仍透传）与移动工作区/设置/PDF 能力相关测试通过。
- Android ARM64 release 构建成功；v3 签名、ZIP 完整性与 ZIP/native 16KB 对齐通过。
- 包名 `com.poco_ai.agentero`，版本 0.15.5/15005，API 33/36，仅 ARM64；FileProvider 与 PDF 分享边界未变。
- 真机重点验收：查看图片后关闭，应用 UI 不再整体放大；双指缩放图片/页面不会改变壳层；单指在文字上滑动只滚动，不弹出选区或明显卡顿；长按 1 秒选择、3 秒备注仍可用。

## 15. 阅读手势、预览与翻译壳层重新打包验证（2026-11-15）

本轮改动（commit `8b6625c3c`）：

- **图片预览**：已识别图/表/算法/公式区域在鼠标/手写笔 hover 以及手指按下时显示放大裁剪预览；点击进入全屏。全屏查看支持双指缩放、单指平移，单击、关闭按钮、ESC 或 Android 系统返回退出。WebView 页面缩放仍关闭，缩放由图片查看器内部处理。
- **长按选择**：1 秒长按选中当前位置的词，滑动手势只滚动；移动端不再因长按自动翻译，也不再弹出 3 秒备注框。点页面空白清空选中；备注/翻译通过现有选区工具栏触发。
- **翻译壳层**：打开双栏译文 pane 时顶部预设同步切到对照；切回阅读或使用覆盖模式隐藏译文 pane。翻译中的页面盖半透明遮罩，完成页恢复正常。
- **工具栏**：移除页面左上角“翻译页面”按钮；右上角框选/视觉批注按钮与翻译按钮统一为平板 44px 触控尺寸；顶部阅读/对照/研讨改为纯图标并缩短。
- **菜单**：桥接主机名为 localhost/127.0.0.1 时显示 `Agentero Bridge`，不再露出 loopback 地址。
- **滚动**：单指 pan 增加短惯性滑行。
- 上一轮的双栏翻译、EasyScholar 设置、图表/备注语义、WebView 缩放禁用等保持。

验证与产物：

- TypeScript 通过；依赖图 1485 模块 / 8180 条边无违规；移动工作区/设置/手写输入/PDF 能力/翻译生命周期/后台任务条等 9 个文件 135 项通过。
- Android ARM64 release 构建成功；v3 签名、ZIP 完整性与 ZIP/native 16KB 对齐通过。
- 包名 `com.poco_ai.agentero`，版本 0.15.5/15005，API 33/36，仅 ARM64；FileProvider 与 PDF 分享边界未变。
- **仍待下一轮**：长按后拖动扩展选区与两端摇杆、滑动首尾 bounce、原文/译文批注与手写双向实时同步。当前版本长按只选一个词，备注/翻译走选区工具栏；滚动只有惯性没有回弹。真机还需验收全屏图片双指缩放与系统返回、翻译预设同步、图标化顶部栏与 hover 预览。

## 16. 滚动惯性抖动修复重新打包验证（2026-11-15）

本轮修复（commit `2c2865f5f`）：

- **速度采样**：单指 pan 释放时不再用最后一个 pointermove 的 1 ms 瞬时速度（高 DPI/合并事件下会放大噪声、方向抖动），改为最近 80 ms 触摸样本的平均速度；手指停下超过 80 ms 再抬起不启动惯性。
- **位置来源**：惯性帧不再每帧读取 `scrollLeft/scrollTop`（异步提交可能导致读回旧值、来回抖），而是维护本地 scroll 累加器并直接写绝对位置。
- **摩擦模型**：按真实时间做指数衰减（约 600 ms 尾巴），不按帧乘固定系数，刷新率或掉帧时速度一致；速度低于阈值立即停止。
- 新手指导入时取消上一段惯性，双指缩放/长按仍优先。
- 参考常见移动滚动实现（UIScrollView / Android OverScroller 类）：速度追踪窗口 + 位置累加 + 时间指数摩擦。

验证与产物：

- TypeScript、Biome 通过；手写输入 18 项、移动工作区 17 项相关测试通过。
- Android ARM64 release 构建成功；v3 签名、ZIP 完整性与 ZIP/native 16KB 对齐通过。
- 包名 `com.poco_ai.agentero`，版本 0.15.5/15005，API 33/36，仅 ARM64。
- 真机重点验收：慢速短滑、快速甩动、手指停顿后抬起、连续多次滑动、双指缩放前是否有残留惯性抖动；首尾 overscroll bounce 仍不在本包范围。

## 17. 最终触摸惯性参数与移除调参入口（2026-11-15）

本轮改动（commit `b1044a118`）：

- 采用实机调参结果，把惯性参数硬编码到 `bindInkInput`：
  - 速度采样窗口 **120 ms**
  - 摩擦系数 **0.001**
  - 停止阈值 **0.005 px/ms**
  - 释放速度倍率 **1.0×**
- 移除临时“阅读手感”设置分区、四个 `pdfInertia*` 设置字段（前端 + Rust schema）、保存并重启按钮以及仅供调参的 Android restart 桥；设置页恢复原导航。旧 settings.json 中残留的四个字段在下次保存时被忽略/丢弃，不影响其他设置。
- 惯性实现保持位置累加 + 120 ms 速度窗口 + 时间指数摩擦，无调参入口。
- 上一轮的 1 秒选词、点击清选、图片预览/全屏、翻译预设同步等保持。

验证与产物：

- TypeScript、Biome 通过；移动设置/手写/工作区/PDF 能力/后台任务条 5 个文件 49 项测试通过；Android ARM64 release 构建成功。
- 包名 `com.poco_ai.agentero`，版本 0.15.5/15005，API 33/36，仅 ARM64；v3 签名与 16KB 对齐通过。
- 真机重点验收：慢速短滑、快速甩动、停顿后抬起、连续滑动是否与调参时一致；设置页已无“阅读手感”入口；旧设置不再弹出/丢失其他配置。

## 18. 原文/译文手写与高亮共享（2026-11-15）

本轮改动（commit `5a44a3e88`）：

- **手写/绘制图形双向实时同步**：`PdfTranslationViewerInner` 也挂载 `usePdfInk`、`InkInputBridge`、`PdfInkLayer` 与 `PdfInkToolbar`。两个视图使用同一 paper key，因此共享同一个 `InkController`；任一侧落笔、擦除、移动、撤销都会立即更新另一侧的 `PdfInkLayer`，并写回同一 `marks/ink/pN.json`。
- `PdfPageLayers` 新增 `inkOverReflow`，允许在译文 reflow 页上绘制/显示 ink；译文的笔工具栏使用译文文档页面尺寸。
- **高亮/批注镜像**：原文把 `PdfHighlight[]` 发布到 paper-keyed 的内存 mirror；译文通过 `translatedHighlightsByPage` 在当前翻译 overlay 上实时绘制。原文新增、改色、改备注、删除后译文同步变化；有 `translatedRects` 时用译文框，否则回退到原文归一化框。
- 边界：标注的**创建/写盘仍由原文的 EmbedPDF annotation plugin 负责**。译文 pane 是无 annotation plugin 的轻量视图，本轮只做实时镜像，不在译文侧新建 PDF annotation；视觉笔记卡片/comment rail 也仍只在原文 pane 操作。手写是完整双向，标注是原文→译文实时镜像。

验证与产物：

- TypeScript、Biome 通过；手写/移动工作区/PDF 能力/设置 4 个文件 46 项测试通过；Android ARM64 release 构建成功。
- 包名 `com.poco_ai.agentero`，版本 0.15.5/15005，API 33/36，仅 ARM64；v3 签名与 16KB 对齐通过。
- 真机重点验收：在原文画一笔，译文立即出现；在译文画一笔/擦除/撤销，原文立即变化且重启后保留；原文新建/改色/删除高亮，译文同步出现/变色/消失；译文 reflow 页的 ink 位置是否与原文页面尺寸一致。

## 19. 译文视图黑屏修复（2026-11-15）

- 原因：译文 pane 复用手写输入桥时，`InkInputBridge` 无条件调用 `useSelectionCapability()`；轻量译文运行时没有 selection 插件，打开翻译即抛错，页面黑屏。
- 修复：`InkInputBridge` 改为接收 owner viewer 传入的可选 `selection` capability，不再自行调用该 hook；原文 pane 传入自己的 selection，译文 pane 不传。手写输入/平移仍可用，译文没有选区清空/长按选词能力。
- 验证：手写输入 18 项、移动工作区 17 项测试通过；Android ARM64 构建成功。
- 真机验收：点击翻译应正常显示译文页，不再黑屏；译文内可手写并与原文同步。

## 20. 译文手写改用原文档唯一工具栏（2026-11-15）

- 问题：译文 pane 也渲染了 `PdfInkToolbar`，两个工具栏各自绑定一个 `InkEditor`；手写笔在两侧切换时，工具状态互相覆盖，出现笔型切换错误。
- 修复：`usePdfInk` 新增 `sharedEditor` 选项，原文与译文两个可编辑 pane 共享同一个 `InkEditor`（每个 paper 一个，只读嵌入仍隔离）。译文 pane 删除自己的 `PdfInkToolbar`，只保留原文档工具栏；笔/荧光笔/橡皮/撤销的模式由这一处统一驱动两侧文档。
- 共享编辑器在最后一个可编辑 pane 卸载时清理，避免 paper 重开时持有旧 controller。
- 边界：窄屏只显示译文 pane 时，原文档工具栏不可见；切回原文后仍可切换工具。译文 pane 本身不提供工具栏入口。
- 验证：新增回归测试确认两个可编辑 pane 拿到同一个 editor 和同一 pen 模式；手写输入、ink viewer、移动工作区 39 项通过。
- 真机验收：译文页无手写工具栏；原文工具栏切到手写/荧光笔/橡皮后，两侧笔型一致；双栏交替书写不再出现工具切换错乱。

## 21. 小米/三星/华为/联想通用笔身按键适配（2026-11-15）

- 在没有厂商企业 SDK 的前提下，新增公开 Android `KeyEvent` 路径：WebView `OnKeyListener` 捕获 Android 14+ 标准 `KEYCODE_STYLUS_BUTTON_PRIMARY/SECONDARY/TERTIARY`，以及 Lenovo/旧 Wacom 常见兼容键码 289/290、600–604，映射为橡皮/上一工具/调色盘。
- 仅当已激活、聚焦且获 Rust token 校验的 PDF 目标存在时消费按键；其他按键返回 `false`，WebView、系统截图和键盘行为不变。没有品牌字符串判断、反射或私有 SDK。
- 已知边界：Lenovo 的 `lenovo.intent.action.INPUT_DEVICE_CLICK_STATE_CHANGED` 是 protected broadcast，第三方无法接收；Samsung Air Actions 需要 S Pen Remote SDK 和 enable key；Huawei/Xiaomi 多数笔身手势不在公开广播/KeyEvent 通道；这些品牌的标准手写、压感、倾斜、接触时侧键仍通过通用 pen 输入工作。
- 真机验收：在上述品牌设备上打开 PDF 并进入手写模式，按笔身主键应切橡皮、次键切上一工具/调色盘（具体取决于固件投递的键码）；无反应时记录 `adb logcat` 中 KeyEvent 的 keyCode 再补映射。

## 22. Android 12 兼容与通知权限门控修复（2026-11-19，已撤回）

> **状态更新（2026-11-19）**：Android 12 支持已撤回，minSdk 与通知门控恢复为 Android 13 / API 33 基线，后续不再支持低于 13 的系统。本节保留为 round 记录；已发布的 v0.16.1 Release（minSdk 31 双包）不受影响，仍可供存量 Android 12 设备下载使用。

背景：客户小米平板 5 Pro 停留在 Android 12，装不上 minSdk 33 的包；更早的本地 release 包因未签名在 MIUI 上报「解析软件包时出现问题 (33) packageInfo is null」（CI 注释与 Tauri 社区案例均印证未签名包不可安装）。本轮把支持基线下探到 Android 12，并保留 Android 13 基线包。

代码与配置改动：

- **通知权限门控按版本分支**：`AndroidBackgroundNotifications.granted()` 原先无条件 `checkSelfPermission(POST_NOTIFICATIONS)`；该权限 Android 13 才存在，12 上恒返回拒绝，导致后台运行开关永远无法开启、状态查询恒报 denied。现 `SDK_INT >= 33` 才走运行时权限检查，低于 13 返回 true，由 `enabled()` 的应用通知开关与渠道重要性继续把关。`requestNotifications` 与 `state()` 都先经 `granted()`，12 上不再发起无效运行时权限请求。
- **minSdk 33 → 31**：应用 Gradle、`agentero-mobile` 插件库 Gradle（清单合并会校验库 minSdk，漏改会在 `processUniversalReleaseMainManifest` 失败）、`tauri.android.conf.json` 三处同步。target/compileSdk 36 不变；CI 的 android cargo check 仍用 API 24 工具链，release 由 tauri CLI 跟随 minSdk（NDK 目标随之变为 android31）。
- 决策 D-And-1 更新为 Android 12 / API 31；Android 13 仍是完整体验基线。

Android 12 上的已知降级（不崩溃）：掌触取消（`MotionEvent.FLAG_CANCELED`，13+）不可用，手写模式手掌搁屏可能留墨；标准笔身按键（14+）与类型化前台服务（14+）本就不在 12/13 提供，12 走普通 `startForeground`（与 13 相同）。前端要求 WebView Chromium ≥ 111，Android 12 需独立更新 System WebView，否则显示原生升级提示页。清单已满足 Android 12 的 targetSdk 31+ 强制项（组件显式 exported、PendingIntent `FLAG_IMMUTABLE`）；12 不识别的权限/FGS 类型被忽略，不影响安装。

产物（本地测试签名，apksigner 按 SDK 21–36 验证 v1+v2+v3 全通过，zipalign -P 16 通过，仅 arm64-v8a，前端与词典入包）：

- Android 13 基线包（minSdk 33，通知门控修复前构建，13+ 行为与修复后一致）：[Agentero-0.16.1-arm64-android13.apk](../../../artifacts/android/Agentero-0.16.1-arm64-android13.apk)，76,909,817 字节，SHA-256 `3d9d206946c880fc18b81215bfae8c2f74d290deb7047e2e9695dd5385f72f84`。
- Android 12 兼容包（minSdk 31）：[Agentero-0.16.1-arm64-android12.apk](../../../artifacts/android/Agentero-0.16.1-arm64-android12.apk)，76,909,817 字节，SHA-256 `8c2497e7a02862ef55822e8f38bf16139978ff0f57a4793b972296eb66856a44`。

验证边界：Rust 按 android31 目标完整编译、Kotlin/清单合并/R8/签名通过；通知门控改动为纯 SDK 分支，JVM 策略测试不涉及 `granted()`（需 Android Context）。**真机验收待客户**：12 上后台运行开关与通知渠道、掌触降级表现、WebView 升级提示、息屏保活；13+ 回归确认门控分支无行为变化。

## 23. 0.17.0 上游同步合并（2026-11-20）

把 mobile 的 0.17.0 合并成果同步进 android。android 先由 `6eaa2041b`（tippy）合并 mobile 至 `e1964f598`，完成 0.17.0 主体与 `input.ts` 三方 reconcile（android 专属输入 noteHold/hoverGuard/pageHitTarget、形状功能、上游 0.17.0 新 input、并行调参 click-guard 4px 等共存）；本轮合并 `3342982fb`「merge: sync mobile 0.17.0 into android」补齐 mobile 剩余 4 个聚焦修复，干净合并、无冲突：

- `86ed3b963` sync 引擎 Path A：恢复 mobile apply-before-publish（mutation lane 内落盘、publish 前），叠加上游 recovery journal/scope.rs/强制条件写/fork 保守合并；journal 落盘前记 intent、apply/upload 失败 discard、CAS 成功 accept+finish。
- `350ac2ab6` 套索视觉笔记路径恢复（beginVisualAnnotation/onVisualDraft 与 region-prompt 并存，视觉 deps 可选）+ 上游 PDF 行为测试对齐（zoom=4、aria-label 含快捷键、isMobileApp mock）。
- `8f9214c08` api-policy 测试 mock 满足上游远程版面页数校验（validateRemoteLayoutPageCount）。
- `f0a5ee42f` 跨视图高亮即时 peer 镜像恢复（挂载即订阅 + 导入前事件缓冲重放）。

android 平台适配全部保留：minSdk 33 / targetSdk 36 / compileSdk 36（Android 12 撤回决定不变）、MainActivity 守卫/insets、FileProvider 边界、清单服务/权限、`agentero-mobile` 插件 Kotlin（后台服务、手写笔适配、WebView 兼容守卫、铅笔快捷、原生导入、外部 PDF intent）及 android 专属 input 层。共享代码（sync 引擎/region-framing/highlights/bindings/i18n）与 mobile `f0a5ee42f` 一致；android 另保留其独有的移动增强（figure-lightbox、mobile-task-indicator、mirror-store 等）。

验证：agentero-core 全量、bindings 只读 + events 契约、android cfg check、Kotlin `:app:compileUniversalReleaseKotlin`、typecheck/deps:check/biome、vitest 广域（含 ink/shape/click-guard/stylus/region-prompt）全绿。产物见下（0.17.0 ARM64 测试签名包，真机验收待用户）。

## 24. dsh 修复分支集成与 0.17.0 测试包（2026-11-20）

出包前把 `dsh-5629cae8-7588` 的 5 个修复提交 cherry-pick 进 android（保留原作者 tippy 与原消息），测试包须含这些修复：

- `447079ffc`→`aa4b638a2` 翻译按钮改为译文可见性开关。**display-mode→view-mode 适配**：上游 0.17.0 已删 `translate.displayMode` 设置，改用 view-mode(original/translated/dual) 单一权威。按钮复用 view-mode 机制——single 视图 toggle 原文↔译文（同 ⌥A）、companion 开则收回单栏原文、运行中则停止；布局切换交给独立控件（桌面新增 side-by-side 工具栏按钮 `handleSelectViewMode("dual"/"original")`、mobile compare 预设）。丢弃 dsh 的「首次点击跟随配置显示模式」（无配置可跟随）与 `display-entry.ts`/`dualLayoutRequested` 死代码；`translationShown` 保留 0.17.0 语义（overlay 跟随 view-mode，original 不画译文）。
- `710d92fa9`→`ad63e8c10` 笔迹页相对、原文/译文共享。主视图不再因译文显示而解除 ink，`inkOverReflow` 让笔迹画在重排页上；译文 companion 接入共享 `usePdfInk(sharedEditor)` + `InkInputBridge` + 页属性，两栏显示/书写/移动/擦除同一笔迹。
- `74b778ae1`→`498787a0d` 同类同论文新任务启动/完成时 retire 旧失败行；桌面面板逐行 dismiss、mobile 失败 pill 可关闭/滑走（含 background-tasks 测试，用户授权分发）。
- `9587c2271`→`b8f1588c8` plaza tab 套件补 workspaceStore mock（DocView 现订阅 workspace store 检测 companion tab）。
- `ad2ff414b`→`9327a0703` `cacheReflowFinal` 默认翻转为开 + 一次性迁移（设置开关仍可关）；settings/mod.rs 文档注释改动→重生成 bindings。

合并遗留修绿（`985b48837`/`09bb4681b`）：0.17.0 同步曾把 mobile 的 EasyScholar 设置块从 android mobile-paper-info 丢掉，恢复共享 `EasyScholarSettingsBlock`（保留 android `openMobilePaperLink`）；android mobile-settings-page 有意新增 EasyScholar 独立导航分区，native-pdf-import 导航断言更新为 7 项并注释；translation-recovery 套件补 viewport/zoom/interaction capability mock（companion ink bridge 新订阅）。

验证：agentero-core 全量 1047/0/9ignored、bindings 只读 + events 契约、android cfg check、Kotlin `:app:compileUniversalReleaseKotlin`、typecheck/deps:check/biome、vitest 广域（含 background-tasks/plaza-tab-persistence/mobile-task-indicator/ink/translate/region/highlight）全绿。产物：0.17.0 ARM64 测试签名包（minSdk 33、v1+v2+v3、zipalign -P 16），真机验收待用户。

## 25. 参考

- 共享部分：[tablet-shared.md](tablet-shared.md)
- DSH Go（`74e1faa`，AGPL-3.0，仅参考设计）：清单里的保活服务声明与权限、使用指南的「后台运行」一节 <https://github.com/ccch1mneyyy/dsh-Go>
- ds-harness-remote 的 Android 前台服务与保活实测记录（`apps/android/plugins/`、`docs/TODO.md`）：<https://github.com/liguobao/ds-harness-remote>
- 前台服务类型：<https://developer.android.com/develop/background-work/services/fgs/service-types>
- 前台服务超时：<https://developer.android.com/develop/background-work/services/fgs/timeout>
- 从后台启动前台服务的限制：<https://developer.android.com/develop/background-work/services/fgs/restrictions-bg-start>
- 低电耗模式与电池优化豁免：<https://developer.android.com/training/monitoring-device-state/doze-standby>
- 用户发起的数据传输：<https://developer.android.com/develop/background-work/background-tasks/uidt>
- 多窗口：<https://developer.android.com/develop/ui/compose/layouts/adaptive/support-multi-window-mode>
- Activity 嵌入：<https://developer.android.com/develop/ui/views/layout/activity-embedding>
- 手写笔进阶特性：<https://developer.android.com/develop/ui/views/touch-and-input/stylus-input/advanced-stylus-features>
- Ink 库：<https://developer.android.com/jetpack/androidx/releases/ink>
- 文档与文件访问：<https://developer.android.com/training/data-storage/shared/documents-files>
- 预测性返回：<https://developer.android.com/guide/navigation/custom-back/predictive-back-gesture>
- Tailwind CSS 浏览器要求：<https://tailwindcss.com/docs/compatibility>
- Tauri / tao 更新记录（移动端多窗口、返回键事件、模板版本）：<https://github.com/tauri-apps/tauri/blob/dev/crates/tauri/CHANGELOG.md>、<https://github.com/tauri-apps/tao/blob/dev/CHANGELOG.md>
