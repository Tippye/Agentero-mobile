# 平板端实施方案：共享部分（Android 平板 / iPad）

> 状态：**开发中，尚未完成整体验收**。现状分析与源码行号基于 `66ecc126`（v0.15.5，Tauri 2.12.5 / tao 0.37.2）；实施进度见下表，不把已写代码等同于真机验证。
> 本篇以 `mobile` 为共用分支，`android`、`ios` 基于它接入平台实现。平台篇各在自己的分支：`tablet-android.md`（`android` 分支）、`tablet-ios.md`（`ios` 分支）。

## 0. 实施进度

| 工作项 | 当前实现 | 尚待完成 |
|---|---|---|
| S0 宿主收敛 | Android/iOS 共用显式命令白名单；桌面状态、服务和本机 Agent 初始化隔离；移动文件权限限定应用私有目录；Bridge 客户端接入类型绑定；Android ARM64 library 与完整 APK 构建、测试签名及 16KB 对齐验证已通过；Chrome 111 以下内核在 React 加载前由原生页面提示更新 | Android 设备验证、iOS 目标构建；继续审计成熟组件的桌面调用链 |
| S1 本地 Vault | 不配对即可创建/恢复应用私有库，使用原生规范路径统一任务与事件身份；本地 Catalog、私有 cache 暂存 PDF 导入；标识/名称魔棒由原生 JobCenter 入库；原生 PDF 选择、描述符流式暂存、Vault 身份复核与原件保留已接入；Android 单份 ACTION_VIEW 打开与 ACTION_SEND 分享 PDF 的原生授权邮箱、前台库就绪领取及同内核入库已接入；iOS 仅限系统复制到应用 Inbox 的单份 PDF，原生描述符邮箱、深链过滤及同内核入库已接入 | iOS 目标 SDK/设备与冷启动验收、原地打开/分享扩展、Android 多份/文本分享、真实文件提供器、平台目录和升级迁移验收 |
| S2 文档读写 | 本地 PDF 使用真实论文路径恢复标注持久化；Plate 笔记自动保存；Markdown 最终写入检查精确磁盘基线，冲突保留原始草稿；关闭时保护最新草稿和仍被原件引用的图片 | Bridge 文献读写统一、远程文件版本检查与安全加固；跨文档图片引用不宣称原子回收 |
| S4 平板工作区 | 三档容器宽度、阅读/对照/研讨预设与公开可拖分隔条；PDF/笔记/Agent 固定 pane，保留字节、位置、草稿与会话；阅读默认关闭辅助面板；宽档论文列表可折叠且不重挂载阅读器，中/窄档保留菜单抽屉；论文行左滑露出操作层（永久删除论文目录并二次确认，不走回收站）；打开/恢复论文固定阅读预设，EasyScholar 密钥移至设置页，长按选词后使用选区工具栏添加备注，图表点击改为可缩放全屏查看；阅读适配宽度不预留桌面批注栏；参考文献可解析；全文翻译按设置打开双栏译文 pane（原文可编辑）或覆盖层（原文只读并显示进度），任务在应用级后台继续；广场标题、来源下拉、刷新与打开浏览器图标合并到一行，移除广场魔棒且浏览器仅图标；左侧文献库/广场/Agent、魔棒与信息/EasyScholar，不含写作；原生系统栏/键盘与四边安全区域 | 实际系统窗口缩放、分屏、键盘与前台视觉验收；后续行操作按钮按同一滑出层扩展 |
| API 阅读处理 | 移动设置页按电脑端结构聚合外观/翻译/排版/同步/后台/关于，侧栏单设置入口；应用内 API 设置、Paddle/MinerU 版面解析及 API 翻译；打开只读缓存、显式提交；移动壳持有两类现有 executor；已完成翻译块/逐页 IR 复用，原生私有异步回执及原始结果持久恢复；文件插件隔离回执和祖先修改；修复 Android ink actor fsync 越过应用目录的保存失败 | 真实 API 与 Android/iOS 前后台/重启验收；未知提交结果不自动重提，无新付费 generation 或收据自动淘汰；同步 MT 不承诺崩溃中 exactly-once |
| S5 远程 Agent | provider 契约、命名空间、能力协商、未知结果控制；DSH REST/Noise/relay/RPC；原生会话/历史/审批/提问/附件映射；类型 IPC 和默认 DSH 移动界面；移动端系统凭据存储、稳定设备身份与连接/指纹恢复、刷新不确定性落盘、清除失败保留账号；Bridge 身份系统迁移、严格恢复与显式重置；原生 SQLite 操作 journal、WebView 重建后恢复 ID 与未知结果保护 | OS 密钥存储与真实账号/Host 的设备闭环；ACP/Codex、DSH 扫码配对与直连目前明确不支持 |
| S3 结构同步 | 批注/笔迹结构合并；原件冲突保留；移动 S3/WebDAV 设置；PDF 按需获取与离线固定、完整下载选择和校验后清缓存；移动凭据迁入系统存储；前台保存静置/间隔/恢复调度；远端命名空间绑定与精确 HEAD 检查，覆盖换库、清单分叉和 CAS 重试 | 真机与真实 MinIO/WebDAV 的端到端和恢复验收；过滤发布者不能证明整篇删除时仍保守保留笔迹；原生后台另见平台阶段 |
| S6 手写数据 | 原生 CAS 笔迹 sidecar、逻辑版本/墓碑与耐久同步；WebView 输入及只读显示、笔/荧光笔/按笔擦除/套索移动复制删除/撤销重做、标准压力 Ink 副本导出；套索区域经预览草稿明确发送到默认远程 Agent，未知结果不重放；笔绘制、手平移/缩放及长按选择、标准侧键/反转橡皮、当前文档绑定的原生动作；Pencil 双击/Pro 挤压的系统偏好路由与可扩展 Android 品牌适配器 | Android 笔/Apple Pencil 真机输入、性能/取消/压力验收；来源回跳、原生分享与平台输入路线取舍 |
| S7/S8/S9 | 按窗口/Vault 保存论文、模式、页码与缩放并在库载入后恢复；Android 双前台服务、iOS 有限窗口、Rust 工作计数/过期取消、只读会话观察和 Android 显式设置已接入；scene 临时中断与多 scene 前台标记有回归；Android 官方系统返回接入顶层弹窗及草稿保护，干净根页恢复系统默认；原生网络变化计数、传输 epoch 隔离与既有观察器只读恢复已接入；iOS 内容进程终止有界重载与旧手写目标同步失效已接入 | 原生多窗口、完成/审批通知与回跳、切网真机验收、分享队列、完整 IPA 和设备后台仍未整体验收；Android 测试 APK 已完成构建签名 |

已完成的检查包括 Android ARM64 library 目标编译、IPC 绑定生成和只读比对、事件契约、provider 与 DSH 原生协议测试、44 项核心测试和 96 项前端测试。两项需要真实 MinIO/WebDAV 服务的既有测试保持忽略。macOS 原生移动预览在移动端私有文件 ACL 下验证了创建/恢复 Vault、目录内写入、版本校验、旧版本拒写、原件保留、越界拒绝、删除和默认 DSH 页面；这不等同于 Android/iOS 真机通过。完整 Xcode/iPhoneOS SDK 在当前环境不可用。

网络恢复阶段另外完成 Rust 全量 908 项检查（898 通过、10 项既有外部环境用例忽略）、原生频道 3 项、绑定只读比对与 9 项事件契约。独立审查发现提示读取可能跨过正常轮询截止时间，现已限制提示预算并用阻塞回归验证；修复后 21 项后台观察、18 项 Bridge、4 项计数接入与 14 项网络筛选检查，以及 workspace/all-targets 严格 Clippy 均通过。Android 20 项 JVM 策略与 iOS Foundation 的 69 项断言不替代实际设备切网或完整 iOS 目标验收。实现边界见[移动网络恢复](connectivity.md)。

原生 PDF 选择阶段复用固定版本的系统 picker，在 Rust 私有 cache 流式暂存后进入同一 commit；Android 整文件描述符边界与可选显示名另由私有原生适配器保证。9 项新宿主用例、移动命令登记、绑定生成/只读比对和事件契约通过；最终宿主全套 918 项中 908 通过、10 项既有忽略；移动端/PDF 共 74 个文件、1246 项测试通过，10 项既有跳过；TypeScript 与依赖图（1477 模块、8109 边）、严格 Clippy 通过。Android Kotlin 编译及 38 项 JVM 策略检查通过。隔离的 `pnpm tauri dev` 已验证真实 IPC 与平板页面初始化；预览窗口无法由桌面控制服务捕获/激活，因此原生选择器的实际取消不计为通过。这些检查不等同于真实 Android/iOS 提供器、系统分享和完整 iOS 目标验收；资源及权限边界见论文入库。

后续凭据与编辑安全阶段另通过 37 项原生测试（包含跨启动、刷新前持久化、未知刷新不重试、失败清除、精确 Markdown 并发写入与凭据缓存隔离）、99 项定向前端测试、全量 TypeScript、依赖检查、只读绑定/事件契约和全工作区严格 Clippy；新的移动凭据路径通过 Android ARM64 交叉编译。Android/iOS 使用仅 Rust 可调用的 keyring-store，iOS 明确配置 `AfterFirstUnlockThisDeviceOnly`；桌面移动预览仍为启动内会话。Android 的 AES-256 与持久化后端补丁留在平台分支，不把这些测试视为真实设备验证。

本轮同步阶段通过 103 项核心测试（另有 2 项真实服务测试保持忽略）、22 项原生同步/凭据/调度测试、9 项事件契约、生成与只读绑定检查、全量 TypeScript、依赖检查及全工作区严格 Clippy；前端有 93 项广域回归及最终 26 项针对同步与阅读器的回归。macOS 原生预览使用工作区内独立配置和数据目录，在 900×866 视口实际执行菜单→同步弹窗→S3/WebDAV 表单切换→下载选择→原生策略读取，并验证未配置时拒绝同步。合入平台分支后，使用 API 33 NDK 的 Android ARM64 library 编译检查通过（保留既有移动端未使用代码警告），iOS 18.2 发布预检通过；当前仍缺完整 Xcode/iPhoneOS SDK。此处为宿主 IPC/DOM 与交叉编译流程，不等同于 Android/iOS 密钥存储、真实服务同步或完整视觉/键盘验收。

远程恢复阶段通过 43 项 Bridge 测试、47 项远程服务/账号/journal 测试（含 14 项实际临时 SQLite 回归）、43 项前端回归、全量 TypeScript、依赖检查、只读绑定、9 项事件契约及全工作区严格 Clippy。Bridge 的缺失、损坏和存储失败均不静默换身份，显式重置先落清除标记；旧 transport 在断开/重置后不会继续发送排队 RPC 或签名证明。移动端操作派发前持久保存 Unknown，重启和回执缺失不触发重发；journal 故障保留只读历史与本地库。macOS 原生宿主在工作区隔离的配置/数据目录与 900×866 视口验证了离线 journal IPC、公开配对 metadata、菜单→重置确认及系统存储不可用时保留配对与错误反馈；这不是设备清除成功的验收。合入后的 Android 分支已通过 API 33 ARM64 library 编译检查（既有 3+16 项移动目标未使用代码警告），iOS 18.2 发布预检通过；iOS 完整构建仍缺 SDK。具体顺序见[远程状态恢复](remote-agent.md)，设备闭环仍未完成。

自适应工作区与笔迹基础阶段通过 140 项联合前端回归、21 项笔迹模型/存储/原生身份回归、108 项同步回归（另 2 项真实服务用例继续忽略）、只读绑定、9 项事件契约、全量 TypeScript/依赖检查及全工作区严格 Clippy。额外最终 8 项回归覆盖导出反馈与笔迹视图。599/600/839/840 边界与各预设的真实 DOM 回归确认源字节、阅读位置、笔记草稿及 Agent 会话不重挂载；独立 PDFium 解析和栅格化了四种裁剪/旋转的压力 Ink 外观。macOS 隔离宿主实际 IPC 验证保存/读取/删除墓碑/恢复、原创建时间、陈旧 CAS 拒绝和标准 Ink 副本，原 PDF 哈希保持不变。该宿主在公开聚焦请求后仍报告文档 `hidden`，阅读器与工具已挂载，但 PDF 页被前台可见性门控，故不宣称完成前台绘制、系统分屏、视觉或真机手写验收；也不修改产品后台策略以绕过门控。套索区域提问、原生分享/多窗口与平台后台阶段继续推进。合入平台分支后，Android API 33 ARM64 library 编译检查通过（保留既有 3+16 项移动目标警告），iOS 18.2 发布预检通过；完整 iOS 构建仍缺 Xcode/iPhoneOS SDK。直接执行冷缓存 Android Cargo 检查时须按生成 Gradle 项目的实际包名设置 `TAURI_ANDROID_PACKAGE_UNESCAPED`（以及 Wry 包名/库名），与 `TAURI_ANDROID_PROJECT_PATH` 一同传入；Tauri CLI 的完整构建会提供这些代码生成参数。

圈选提问阶段已接通套索→源 PDF 区域截图→默认远程 Agent 参考草稿→明确上传与发送。无笔迹图表区域同样可圈选；来源快照和当前工作区复核避免迟到截图换错论文，已有问题与附件保留。上传和发送分别走原 journal，未知结果不重放，已确认上传在发送被确定拒绝后复用。参考截图可点开放大查看，元数据降级须明确关闭图片。联合 27 个文件的 193 项移动/笔迹/区域回归已通过，最终放大预览变更另通过 8 项渲染草稿回归、全量 TypeScript、1461 模块/7973 条依赖检查与定向 Biome。隔离 macOS Tauri 宿主实际验证私有缓存 PNG 写入/读取、幂等清理和协商字节上限拒绝；真实 WebView 配合模拟 provider 与实际原生文件插件通过放大预览→输入问题→双击发送→一次上传/发送→来源/不同操作 ID 校验→确认清空和缓存清理的 DOM 流程。未联系真实 Host，仍不宣称前台 PDF 像素绘制、触笔、真实 Host 接收或来源回跳验收。旧 Bridge 图像附件、原生多窗口/分享和平台后台继续推进。机制见[圈选区域提问](paper-context.md)。

平台后台基础阶段接入 Rust 实际工作计数、单调 generation、iOS 过期归零屏障和 25 秒只读会话观察；Host 选择代际避免迟到的旧会话读回切当前 Host。Android 使用独立 `specialUse` / `dataSync` 服务、非黏性生命周期、划掉停止、通用通知与显式设置；iOS 持有同一个有限 UIKit 窗口直至工作结束/取消/到期，不因前后台往返续时，Agent 不申请常驻。新增 35 项 Rust 回归、9 项事件契约、生成和只读绑定、全工作区严格 Clippy、21 文件 163 项移动前端回归、全量 TypeScript 及 1463 模块/7987 条依赖检查通过。Android 原生 Kotlin 2.2.10 编译、10 项 JVM 测试、AAR 与应用清单合并通过；iOS 纯 Swift/Foundation 运行测试及语法解析通过。隔离 macOS Tauri 宿主实际验证不支持平台拒绝启用，以及模拟 OS 下的真实 WebView 显式授权、只启用一次、真实状态展示、电池入口和禁用；未改变实际原生偏好、未联系真实 Host。前台渲染、APK/IPA、真机后台/网络/通知回跳与完整 iOS SDK 验收仍未完成。机制见[平台后台适配](background.md)。

阅读体验补齐阶段完成四边安全区域、Android 原生系统栏/键盘避让、笔手分流与长按选择、当前可见文档的笔手势绑定、默认关闭右侧面板，以及广场/魔棒/论文信息与 EasyScholar。30 文件 248 项联合回归加新增窄屏导航回归（共 249 项）、全量 TypeScript、1474 模块/8097 依赖检查、事件/生成绑定契约及严格 Clippy 通过。原生笔槽位、私有 Vault 规范身份、取消后的部分入库、缺失桌面插件、移动 cache 暂存、Tauri 原生 JSON/Raw 频道解码均有针对性回归。隔离 macOS Tauri 宿主实际导入/读回私有 PDF、验证信息与魔棒弹窗、手动研讨和广场返回当前论文；文档仍为 `hidden`，未验证 PDF 像素或真实触笔。Android ARM64 完整 release APK 已构建，沿用原测试签名，通过 v3 签名、ZIP、ABI、16KB 原生与 ZIP 对齐检查；最低 API 33、target 36。iOS 18.2 预检、Swift 策略和语法检查通过，完整 SDK 与设备仍不可用。厂商笔身双击没有通用接口，保留编译期适配器扩展，不宣称具体品牌 SDK 已接入。详见[移动前端](frontend.md)、[安全区域](safe-area.md)及[手写笔适配](stylus.md)。

系统返回阶段复用 Tauri `onBackButtonPress` 和当前 Radix 关闭协议，覆盖弹层/侧栏/会话历史、阅读预设、笔记与笔迹退出保护；干净根页不持有返回回调，仍由 SDK/系统处理后台与任务根。37 文件 302 项移动/PDF 联合回归、TypeScript 与 1477 模块/8108 依赖检查通过。新增回归覆盖真实嵌套 Dialog、扫码视频 portal 实际挂载后启动、迟到相机控制器停止、根页迟到 portal、严格模式与迟到 Native listener 清理，以及隐藏 Bridge Agent 保留请求/草稿且不显示弹窗。公共 SDK 注册/移除的异步交接、真实预测返回与输入法顺序仍需 Android 设备验证，不用退出进程替代系统返回。审查另补齐根页导入期间不吞返回、RSS 菜单与翻译卡片先关闭、相机 MediaStream 在 ZXing 控制器产生前的回收与父页面更新不重启相机。隔离 macOS Tauri 宿主实际验证干净根页、信息弹窗关闭后保留同一论文及 PDF 哈希、真实 IPC 检查点后只关闭手动配对内层。该 WebView 仍为 `hidden`，退出动画停在 0ms、闭合 portal 未回收，父层保持 `aria-hidden`；第二次父层关闭和最终 backdrop 清理未验收，聚合日志保留不通过结果。不改产品动画/可见性以绕过门控，也未打开真实相机、连接 Host 或宣称 Android 硬件返回已通过。

## 1. 目标与范围

现有移动端是配对桌面后才能用的伴侣端（见 [../frontend/mobile.md](frontend.md)）。本方案把它扩展为可以独立使用的平板应用：论文、笔记、批注存在平板本地，离线可读可写，经 S3 / WebDAV 与桌面同步；Agent 不在平板上运行，由远端电脑提供。

### 1.1 已定的产品决策（2026-11-11）

| # | 决策 | 对方案的影响 |
|---|---|---|
| 1 | 不做写论文功能，不实现 TeX | 平板不注册 `writing_*`、`rtex_*`、`job_latex_compile_enqueue`；`.tex` 只按普通文本只读打开 |
| 2 | 仅 Android 申请后台常驻，参考 DSH Go 的保活设计 | Android 提供权限引导、前台服务与通知；iOS 不提供常驻申请，只使用系统允许的有限后台任务和前台恢复 |
| 3 | 不在平板本机运行 Agent 或命令行，不做 Word 自动化 | 平板不注册 Agent 安装、诊断、终端、`word_*` 相关命令 |
| 4 | 「委派给远端 Agent」与 ds-harness-remote 合并 | 平板只有一个「远程 Agent」功能；委派就是在远端主机上创建会话后离开，稍后回来取结果 |
| 5 | 不做托盘等桌面专属功能；适配 iPad 与安卓平板的分屏和多窗口 | 布局按窗口宽度自适应；支持「在新窗口打开论文」 |
| 6 | 以 `mobile` 为共用分支 | 共享工作先提交到 `mobile`，再合入 `android`、`ios`；平台配置与实现留在对应分支 |
| 7 | 远程 Agent 默认 ds-harness-remote，预留可扩展模块接口 | 核心与 UI 面向统一 provider 契约，新增后端只注册适配器；不要求运行时插件加载 |
| 8 | 体验和功能优先，可放弃较低系统版本 | 方案最低版本为 Android 13（API 33）、iOS/iPadOS 18.2；平台能力仍按实际硬件和运行时检测 |

### 1.2 功能归类

| 类别 | 功能 |
|---|---|
| 直接使用 | PDF 阅读（渲染、搜索、目录、缩放）；文献元数据、标签、筛选排序、本地 Catalog；Markdown 解析、公式、双链索引；引用格式化与 BibTeX / RIS；联网检索与元数据补全；翻译请求；S3 / WebDAV 同步算法；配对与加密通道 |
| 改造后使用 | 笔记编辑（Plate 适配触控）；标注读写（走统一文档接口）；Vault 位置（应用私有目录）；同步调度；凭据存储；文献库与工作区布局；导入与导出入口；远程 Agent（ds-harness-remote 为默认通道） |
| 新增 | 手写笔批注与可合并的笔迹格式；按论文按需下载与离线集合；本地草稿与冲突提示；系统分享入口；分屏自适应与多窗口；后台保活与通知；移动端独立的命令与权限白名单 |
| 不做 | 写作与 TeX；本机 Agent / CLI / 终端；Word 自动化；Git 同步与 SSH 远程 Vault；插件运行时；对外 MCP 服务；平板充当 Bridge Host；托盘、系统菜单、应用内更新、命令行工具 |

## 2. 现状与先决缺口

下面这些问题要先解决，否则后续功能没有地基。归属列对应第 4 节的工作项。

| 缺口 | 位置 | 归属 |
|---|---|---|
| Android 走 `not(ios)` 分支，注册的是桌面 Host 命令；Bridge 客户端命令只在 iOS 分支注册，`bridge_resume` / `bridge_status` 函数本身也限定 iOS | `src-tauri/src/app/handlers.rs:173`、`:333`；`src-tauri/src/integration/bridge/client_commands.rs:27`、`:46` | S0 |
| iOS 仍展开整个 `common_commands!`，包含写作、Word、SSH 远程等平板不需要的命令 | `src-tauri/src/app/handlers.rs:335` | S0 |
| Android 还初始化了文件监听、连接器、Word、MCP、SSH 远程等桌面状态 | `src-tauri/src/app/mod.rs:170` | S0 |
| 通用 capability 没有平台限定，移动端也拿到 `$HOME/**` 文件作用域 | `src-tauri/capabilities/default.json` | S0 |
| Bridge 客户端命令用字符串调用，不进类型绑定，注册漏了编译期发现不了 | `src/lib/bridge/client.ts:53` | S0 |
| Android 配置目录落到 `$HOME/.config` | `crates/agentero-core/src/paths.rs:27` | 平台篇 |
| 移动阅读器不传 `paperAbsPath`，高亮既不加载也不保存 | `src/components/mobile/mobile-reader-page.tsx:178`；`src/components/viewer/pdf/hooks/use-pdf-highlights.ts:291` | S2 |
| 笔记是纯文本框，手动整文保存，没有版本检查；切到 Agent 页会卸载阅读页 | `src/components/mobile/mobile-reader-page.tsx:84`；`src/components/mobile/mobile-app.tsx:193` | S2、S4 |
| 单栏和双栏两套阅读器同时挂载，靠 CSS 隐藏 | `src/components/mobile/mobile-reader-page.tsx:127` | S4 |
| PDF 缓存要先联网问版本才查本地；论文列表只在内存 | `src/lib/bridge/pdf.ts:25`；`src/components/mobile/hooks/use-mobile-papers.ts:20` | S2 |
| 同步对标注文件按修改时间后写覆盖 | `crates/agentero-core/src/features/sync/engine.rs:554` | S3 |
| 同步凭据是明文 JSON；移动端取不到官方账号令牌 | `crates/agentero-core/src/features/sync/config.rs:457`；`src-tauri/src/integration/sync/commands.rs:67` | S3 |

## 3. 总体结构

```text
┌──────────────── 一个进程 ────────────────┐
│ 窗口 1（WebView）  窗口 2（WebView） …    │  React：文献库 / 阅读 / 笔记 / Agent
│        │ invoke / event      │           │
│ ───────┴─────────────────────┴────────── │
│ Rust Host（唯一的数据所有者）             │
│  ├ agentero-core：Vault、Catalog、同步    │
│  ├ 远程 Agent 客户端（dsh-remote / Bridge）│
│  └ 平台适配接口                          │
│        │                                 │
│ 原生插件（Kotlin / Swift）               │  密钥、文件、分享、后台、通知、手写输入
└──────────────────────────────────────────┘
```

三条约束贯穿全篇：

- **数据只归 Rust 管。** 一个 Vault、一个 SQLite、一个同步引擎、一条远程 Agent 连接。窗口只是视图，通过事件订阅变化。这样多窗口不会出现两个写者。
- **界面不直接碰平台差异。** React 只调用 Tauri 命令；平台差异收在一个本地 Tauri 插件里（第 5 节）。
- **共享代码只写一遍。** S 开头的工作项先在 `mobile` 实现、验证，再分别合入 `android` 与 `ios`。平台分支发现共享问题，先把独立修复提交回 `mobile`；不整支反向合并平台配置。共享文档在 `mobile` 维护，两端随合并同步。

```text
main (66ecc126)
└── mobile：共享方案、核心、UI、远程 provider 契约
    ├── android：Android 配置、原生适配、平台方案
    └── ios：iOS/iPadOS 配置、原生适配、平台方案
```

平台篇及导航入口只在对应分支增加；`mobile` 不链接尚未存在的平台文档。三个分支均为本地分支，本阶段不推送。

## 4. 共享工作项

### S0 移动端宿主收敛

- 把 `common_commands!` 拆成几组：核心（Vault、Catalog、论文、Markdown、翻译、搜索、同步）、桌面专属、Bridge Host、Bridge 客户端、移动专属。移动端只注册「核心 + Bridge 客户端 + 移动专属」。Tauri 构建已提供 `cfg(mobile)` / `cfg(desktop)`（`src-tauri/src/app/mod.rs:46` 已在用），门控统一改用它，不再逐处写 `target_os`。
- `app/mod.rs` 里的桌面状态（文件监听、连接器、Word、MCP、SSH 远程）改为桌面专属。
- 新增 `src-tauri/capabilities/mobile.json`（`platforms: ["android", "iOS"]`），文件作用域只含应用私有目录；`default.json` 限定到桌面平台。
- Bridge 客户端命令接入 specta，前端改用 `bindings.ts` 里的类型化调用。按仓库约定重新生成并提交绑定：`AGENTERO_UPDATE_BINDINGS=1 cargo test -p agentero export_typescript_bindings`；新增事件同步登记到 `events_contract.rs`（见 ../backend/api.md）。
- CI 现有 `rust-mobile` 任务只检查 Android cfg（`.github/workflows/ci.yml:82`），补一个 iOS cfg 检查。

验收：两端启动后，前端调用的每个命令都已注册；平板调用写作、Word、SSH 命令返回未注册。

### S1 本地 Vault

- 首次启动不再强制配对（现在未配对直接进配对页，`src/components/mobile/mobile-app.tsx:109`），改为创建或打开本地 Vault 后进文献库。`vault_create` / `vault_ensure` / `paper_list` 已在核心命令里。配对桌面降为设置里的可选项。
- Vault 根目录由平台篇决定，统一放应用私有存储。不把活动的 SQLite 放进云盘或外部文件提供器目录。
- 移动端没有文件监听。回到前台、同步完成后触发一次重扫；同步引擎拉取后本来就会 `rebuild_from_disk` + `prune_missing`。
- 导入：文件落盘 → 识别 DOI / arXiv 编号 → 在线补全元数据。全文解析和 OCR 不在平板本地做（移动端构建已排除 liteparse，`crates/agentero-core/Cargo.toml:64`），这些产物来自同步或远端。标识符识别需要从 PDF 首页取文字，可用阅读器已有的 PDFium WASM；这一步的可行性在 M1 开头验证。

验收：飞行模式下新装应用，能建库、导入一份本地 PDF、打开阅读。

### S2 统一文档接口

现有 `src/lib/vault/fs.ts` 已经按路径前缀在「本地文件」和「SSH 远程 Vault」之间分发（`remote:` 句柄，`src/lib/vault/remote/remote-vault.ts`）。沿用这个机制：

- 本地 Vault：路径就是真实路径，标注、笔记的现有读写代码直接可用。
- 配对桌面的 Vault：新增 `bridge:` 句柄，读写分发到 `bridgeRpc`。移动阅读器把 `paperAbsPath` / `vaultPath` 传给 `PdfViewer`，高亮的加载与保存随之生效。
- `src/lib/pdf/selection/marks-io.ts:44` 直接调用了 plugin-fs 的 `readDir`，改走 Vault 层的目录列举，才能经过分发。

配套修正：

- 笔记改用桌面的 Markdown 编辑器和自动保存管线（`src/lib/workspace/persistence.ts:472`），该管线已有磁盘冲突检测（`:332`）。Bridge 的 `vault_write_text` 增加「期望版本」参数，版本不符时拒绝写入。
- 缓存优先：论文列表和 PDF 版本信息落盘，离线先用缓存，联网后再校验。
- Bridge 传输：桌面端每个分块都重新计算整份文件哈希（`src-tauri/src/integration/bridge/host.rs:1137`、`:1171`），改为一次传输只算一次。
- 保留 Bridge 就要先补三个安全缺口：撤销设备后关闭它的现有连接（`host.rs:339`）；写入前检查末级符号链接（`host.rs:1189`、`:817`）；客户端 RPC 加超时，并等认证完成后再发送排队请求（`src-tauri/src/integration/bridge/client.rs:166`、`:279`–`:358`）。

验收：本地库与配对库两种来源下，高亮在重启后都能恢复；两台设备同时改同一篇笔记，后保存的一方看到冲突提示而不是静默覆盖。

### S3 同步

引擎在 `crates/agentero-core/src/features/sync/`，两端与桌面共用（见 [../backend/sync.md](sync.md)）。要改四处：

1. **触发方式。** 桌面调度器依赖文件监听、定时器和退出时补同步（`src-tauri/src/integration/sync/scheduler.rs`）。抽出一个不依赖 WebView 的入口，移动端的触发点是：回到前台、本地保存后延迟触发、手动、平台后台任务回调。
2. **凭据。** `sync.json` 的明文凭据改经密钥存储接口读写，平台篇各自实现。
3. **按论文按需下载。** 现有同步范围按设备、按类别整体开关，文档写明按论文下载未实现。平板需要：元数据、笔记、标注始终同步；PDF 打开时或「离线保存」时才下载。做法是在本地记一份「已固定」路径集合，未固定的远端条目保留在清单里但不落盘，且缺失不当作删除——与现有范围过滤的不变式相同。注意版面分析缓存 `layout.json` 在 `source/` 类别下（`snapshot.rs:93`），需要把它从 `source` 里单独划出来，否则关掉 LaTeX 源码同步会连带丢掉它。
4. **标注的结构化合并。** 目前只有 `.md` 和 `.table.json` 在双侧修改时保留两份，其余按修改时间覆盖（`engine.rs:554`–`:574`）。为 `marks/annotations.json` 和笔迹文件（S6）增加按条目 ID 合并：两侧并集，同 ID 取较新，删除用墓碑记录。

两个限制要写进发布说明：Git 同步依赖系统 `git`，平板不提供；官方云同步需要账号令牌，移动端目前取不到（见 D3）。

验收：用引擎测试里现成的内存后端（`MemStore`）补合并用例——两端各加一条高亮后同步，两条都在；一端删除、另一端未动，删除生效且不复活。

### S4 平板工作区界面

共享实现已接通容器三档、三种预设与可拖分隔条，PDF/笔记/Agent 保持固定 pane；源字节、阅读位置、草稿和会话的状态保留已有真实 DOM 回归。独立原生宿主与设备验收分别记录，不能用节点测试代替真机键盘、系统分屏与触笔验证。当前工作区说明见[移动端前端](frontend.md)。

- **按窗口宽度分三档**：窄（< 600）、中（600–839）、宽（≥ 840）。用根容器的实际宽度判断，不看设备类型。现在用 CSS 断点同时挂两套阅读器的写法改为按档位只渲染一套。
- **三种预设布局**：阅读（PDF 全屏）、对照（PDF + 笔记或译文）、研讨（PDF + Agent）。分隔条可拖，窄档退回单栏加切换。文献库在宽档用列表 + 详情两栏。
- **复用**：`PdfViewer`、Markdown 编辑器、桌面 Agent 面板的消息渲染。不引入 Dockview。
- **触控**：移动壳自己的控件命中区不小于 44 pt / 48 dp，在业务层包一层，不改 `src/components/ui/` 的通用组件；右键和悬停菜单改为长按菜单加显式按钮；阅读、文字选择、框选、书写四种模式显式切换。左缘返回手势（`src/components/mobile/mobile-gestures.tsx:73`）在书写模式和笔输入时停用。
- **键盘**：软键盘弹出时保持光标可见、阅读位置不跳；外接键盘沿用现有快捷键。
- 文案走 `src/i18n/locales/{en,zh-CN}/mobile.json`。

验收：窗口在三档之间来回缩放，阅读页码、缩放比例、未保存草稿都不丢。

### S5 远程 Agent

**一个功能，可扩展的后端模块。** ds-harness-remote 是默认 provider；Bridge 是兼容适配器。统一契约放在 Rust 的共享功能层，前端通过生成的类型调用它，不直接依赖某个 provider 的 RPC 名称。新增 provider 不改聊天、任务和后台生命周期的业务流程。

计划新增 `features/remote_agent/` 契约与服务，使用 `Send + Sync` 异步 trait 和 `Arc<dyn RemoteAgentProvider>` 注册表；具体实现放在 `integration/dsh_remote/` 等集成层。借鉴现有 `features/agent/remote_host.rs` 的依赖倒置方式，但不复用其中 SSH 启动 CLI 的接口。当前 Cargo 的 `desktop` feature 也被移动构建使用，S0 必须核对 feature 与 target cfg，避免用名字直接判断平台。

| 契约部分 | 约定 |
|---|---|
| 身份与注册 | 稳定的 `providerId`、配置版本、连接实例 ID；所有主机、工作区、会话引用带 provider/连接命名空间，避免 ID 碰撞 |
| 核心操作 | 列主机与工作区、列/建/读会话、发送与取消、订阅事件、审批与结构化提问响应、附件上传 |
| 能力协商 | 返回实际支持的操作、认证方式、后端类型、附件限制；可选操作返回 `Unsupported`，UI 按能力展示，不假定 Bridge 与 DSH 完全等价 |
| 事件与恢复 | 统一消息、任务状态、权限请求、提问、连接状态；保留来源 ID 和游标，重连读取官方历史/快照并去重 |
| 写入语义 | 使用操作 ID；有回执能力时查回执，无回执且结果未知时显式标记未知，禁止自动重发发送、审批或上传 |
| 生命周期 | connect/disconnect/resume 与订阅取消；Rust 管连接，每个 provider/主机连接供多个窗口共享，Android 服务不依赖 WebView JS |
| 扩展边界 | 内建模块注册，保留未来第三方后端接入点；本阶段不做动态插件运行时，不暴露任意命令执行接口 |

DSH 原生会话默认走 `harness.remote.*`。ACP 与 Codex 属于独立协议域，依据服务端能力和工作区可用后端映射，不混用会话 ID；不采用已移除的 `sync.from` 等旧恢复协议。DSH Remote 只替换远程 Agent/委派通道，不替代本地 Vault、S3/WebDAV 同步或 Bridge 文献接口。

首批适配器：

| 通道 | 电脑上需要 | 能力 | 定位 |
|---|---|---|---|
| ds-harness-remote | DeepSeek Harness 桌面版 + `ds-harness-remote` 插件 | DSH 会话；其 ACP 网关另支持 Cursor、Claude、OpenCode、Pi、Hermes、Antigravity（上游标为实验性） | 默认 |
| Agentero Bridge | Agentero 桌面版 | 任意已注册的 ACP Agent；论文上下文工具；远程访问桌面 Vault | 保留，供已配对用户使用 |

**委派的合并。** 桌面的 `agentero-delegate`（见 A2A.md）依赖本机 Agent 和 SSH，平板不做。平板上的委派就是：在远端主机建一个会话、发出任务、离开；主机继续跑，平板稍后重连，从会话列表和历史里取结果。界面上是一个「任务」列表，显示每个会话的状态。

**客户端实现放在 Rust**，新建 `src-tauri/src/integration/dsh_remote/`，与现有 `integration/bridge/` 并列。不直接用上游的 TypeScript 包，理由：

- 连接要在 WebView 被挂起时继续存活（Android 前台服务只能保住进程，保不住被节流的 JS）。
- 设备私钥和令牌应放在系统密钥存储里，不放 WebView 存储。
- 上游只有 `@dsh-remote/protocol` 发布到了 npm，客户端、加密、传输三个包没有发布，本来也得搬源码。
- 服务器对每个客户端设备只允许一条主机连接，多窗口必须共用一条。

实现范围：REST（`reqwest`）、控制通道 WebSocket（`tokio-tungstenite`，已是依赖）、Noise IK 握手（新增 `snow` 依赖）、分帧、RPC 与事件。第一版只走中继；局域网 / P2P / TURN 直连后置。用上游仓库的固定向量（`fixtures/crypto/v1`、`fixtures/protocol/v1`、`fixtures/pairing/v1`）做一致性测试。

**许可**：ds-harness-remote 是 MIT，可移植并在第三方声明里署名。DSH Go 是 AGPL-3.0，只参考设计，不拷贝代码，包括它的 Dart 版 `dsh_remote`。

**登录与信任**：

- 首选扫码配对（上游 device-pairing v1，不需要账号）。它只在托管服务器上可用——开源的自部署服务器没有配对接口，只有账号密码登录；上游文档也写明双端真机联调尚未完成。
- 备选：系统浏览器里完成账号登录；自部署服务器用账号密码。
- 首次连接固定主机公钥并展示指纹，之后公钥变化一律拒绝。首次固定时信任的是服务器分发的公钥，这一点在界面上说明。
- 服务器地址默认只接受 HTTPS；明文 HTTP 仅限回环和私网地址，与上游客户端一致。连接由 Rust 发起，不受系统对 WebView 明文流量的限制，所以这条规则要在 Rust 里自己执行。

**论文上下文。** 桌面靠挂在会话上的 `agentero-context` 工具让 Agent 按需读论文（见 ../backend/context.md）；ds-harness-remote 的 ACP 网关不允许挂 MCP。平板改为：

- 提示词内嵌最小证据（标题、页码、引文），即现有上下文计划的「仅提示词」模式。
- 需要全文时，把 PDF、笔记或选区截图上传到主机会话工作区，再在提示词里引用返回的路径。中继、TURN 路径下单文件上限通常是 20 MiB，局域网 / P2P 更高，以主机回复为准。
- 回答里不再有工具产生的可点击出处；选区编号到本地位置的映射仍由前端保留。
- 结果写回 Vault 只通过用户点「保存到笔记」，远端不能写平板的 Vault。

**事件与交互**：会话事件映射到现有聊天行模型（`src/components/mobile/chat-lines.ts`）。审批和结构化提问两个通道都要支持；Bridge 目前只转发 8 种事件，不含提问（`host.rs:42`），一并补上。结果未知的写操作（发送、审批）断线后不自动重发。

验收：平板建会话并发出任务后切到后台，回来后能看到完整历史；等待审批的请求在重连后仍可处理；另一台设备不能抢答。

### S6 手写笔迹

两端和桌面共用一套数据格式。共享层已接通原生 CAS sidecar、只读显示、WebView 指针输入、笔/荧光笔/按笔擦除/套索移动复制删除、撤销重做，以及带标准 Ink/压力外观的副本导出；同步发布沿用其文件和父目录 flush 保证。套索区域已接通默认远程 Agent：包括无笔迹图表圈选、源 PDF 有界截图、参考预览与明确发送、受限私有暂存、未知操作保护及确定拒绝后的已上传附件复用。草稿不修改 PDF 或创建空白批注，截图不包含独立压力笔迹层；旧 Bridge 图像通道、来源回跳、设备压力/取消/延迟与原生输入路线取舍仍待平台验收。不能把 macOS 模拟指针视为 Apple Pencil 或 Android 笔验证。机制见[PDF 笔迹](pdf-ink.md)与[圈选区域提问](paper-context.md)。

- **笔迹模型**：每笔有 UUID、页码、工具（笔 / 荧光笔）、颜色、基准宽度、点序列（坐标，可选压力、时间、倾角）、创建与更新时间。坐标用 PDF 页面空间（旋转前），与现有标注矩形一致，不存屏幕像素。
- **文件**：`papers/<id>/marks/ink/p<页码>.json`，内容是版本号、笔迹数组、删除墓碑数组。按页分文件，控制单文件大小和冲突面。
- **同步**：按笔迹 ID 合并（S3 第 4 点）。
- **显示**：`PdfViewer` 里加一个只读的笔迹层，桌面也能看到平板写的批注。
- **与 EmbedPDF 的关系**：`@embedpdf/plugin-annotation` 2.14.4 带 ink 标注处理器，但标准 PDF Ink 标注不带逐点压力。它用作导出目标——「导出带批注的 PDF 副本」时把笔迹写成标准 Ink 标注，原始 PDF 不动。
- **工具**：笔、荧光笔、按笔擦除、套索（选中后移动、复制、删除）、撤销重做。默认笔写字、手指滚动缩放，设置里可改。
- **圈选提问**：闭合笔迹或套索取外接矩形，接到现有区域选择和视觉批注流程（`src/components/viewer/pdf/layers/region-select-layer.tsx`），再发给 Agent。
- 手写转文字 / LaTeX 不在首个版本。

输入层有两条路线，由 M0 的技术验证按平台分别选定：A 是 WebView 里的指针事件加画布；B 是盖在 WebView 上的原生书写层，落笔完成后转换成上面的模型。两条路线的存储和显示完全一样。

验收：在平板写的批注，缩放、旋转页面后位置不偏；同步到桌面后可见；两台平板在同一页各写几笔后同步，全部保留。

### S7 分屏与多窗口

阅读路由恢复已接通：Rust 按调用窗口与 Vault 隔离、版本 CAS、私有原子落盘；前端等库载入后恢复论文、模式、预设、页码与缩放，不覆盖用户在等待期间的导航。5 项 Rust 存储回归、91 项联合前端回归、绑定/事件/Clippy/TypeScript/依赖检查通过，隔离 Tauri 宿主实际验证原生读写、陈旧 CAS 拒绝和 WebView 协调器恢复后再保存。独立 Activity/Scene、聚焦确认、系统回收映射与设备验收仍待实现；不得把注册表中的标签当作新系统窗口已成功的证据。

分两级：

1. **窗口可缩放**（系统分屏、侧拉、自由窗口）。必须支持，靠 S4 的三档布局实现，缩放时不重置状态。
2. **多个应用窗口**（「在新窗口打开论文」）。桌面已有文档窗口（`doc_window_open`，`src-tauri/src/app/window/commands.rs:390`；`src/components/shell/doc-window-root.tsx`），窗口标签由路径派生。仓库固定 Tauri 2.12.5 的移动窗口创建有平台前提：Android 必须显式选择合适的独立 Activity，iOS 必须配置且实际支持多 Scene。默认 builder 可能复用现有容器，注册表标签或返回成功不能证明创建/聚焦独立系统窗口。当前先实现按窗口的阅读路由持久化；原生映射、原子去重和回收恢复仍需技术验证，见[路由恢复与 SDK 边界](route.md)。

规则：

- 同一篇论文不开两个窗口，再次打开时聚焦已有窗口（标签由路径派生，天然去重）。不同论文可以并排；论文和它的笔记可以分在两个窗口。
- 系统可能回收并重建窗口。每个窗口的路由（论文、页码、缩放、模式）按窗口标签落盘，重建时恢复。
- 移动端 capability 的窗口列表要包含 `doc-*`。

验收：两篇论文并排阅读；一个窗口加的高亮，另一个窗口的文献库列表状态同步更新；系统回收后重新进入，回到原页码。

### S8 后台与通知

已接通共享 Rust 工作所有权、只读会话观察及四个状态/权限设置绑定；平台分别提供 Android 独立服务和 iOS 有限窗口。原始 `begin/end` 以 Rust 内部 RAII guard 实现，不向 WebView 发放可伪造的 OS 令牌。代际隔离、有限计数归零屏障、选 Host 后旧观察不得复活与临时 scene 中断均有回归；机制和验证边界见[平台后台适配](background.md)。完成/审批/提问通知和点击回跳、原生网络监听及真实设备后台验收仍待后续。

后续范围和接口要求：

- **任务可恢复。** 同步已有上传断点（`pushed.jsonl`）；下载按块续传；导入队列落盘。任何后台任务被系统中止后，下次运行从断点继续。
- **后台能力接口**：先查询 `backgroundCapabilities`（常驻连接、有限任务、后台传输）。仅 Android 暴露常驻申请及 `begin(原因) → 令牌` / `end(令牌)`，原因是 Agent 会话、同步、传输。iOS 返回常驻不支持，不显示申请入口，有限后台任务单独调度，不能返回伪成功的保活令牌。
- **通知接口**：种类为 Agent 完成、失败、等待确认、向你提问，以及同步失败。锁屏上只显示泛化文案，不带正文。
- **Agent 与后台**：任务在远端主机上跑，不依赖平板在线。回到前台后重连并重新订阅，按快照补齐。

### S9 导入、分享、导出

- 接收：PDF 文件、链接（arXiv、DOI、普通网页）、图片，都进同一个导入队列。
- 导出：笔记 Markdown、带批注的 PDF 副本、BibTeX / RIS、单篇论文目录的压缩包。
- 系统入口（分享面板、文件选择器、「打开方式」）在平台篇。

## 5. 平台适配接口

新建一个本地 Tauri 插件（建议路径 `src-tauri/plugins/agentero-mobile/`，Rust 定义命令，`android/` 放 Kotlin，`ios/` 放 Swift），只在移动端注册。接口两端一致：

| 能力 | 接口 | 用途 |
|---|---|---|
| 密钥存储 | 读、写、删 | Bridge 身份、同步凭据、远程 Agent 身份与令牌 |
| 文件导入 | 选文件并复制进 Vault；取分享收件箱 | S1、S9 |
| 文件导出 | 另存为；系统分享 | S9 |
| 后台能力与常驻 | 能力查询；Android 专用申请、begin / end | S8；iOS 不提供常驻入口 |
| 后台任务 | 登记同步回调 | S3 |
| 通知 | 申请权限、发送、点击回跳 | S8 |
| 网络变化 | 订阅 | S5 断线后立即重连 |
| 手写输入 | 开关书写层、同步页面变换、回传笔迹 | S6 路线 B |
| 窗口 | 新窗口、回收与恢复事件 | S7 |

## 6. 里程碑

只排依赖顺序，不估时间。

| 里程碑 | 内容 | 出口条件 |
|---|---|---|
| M0 地基与技术验证 | S0；两端平台基础项；三项验证：手写输入路线、移动端多窗口、Noise 握手对上游向量 | 两端启动进入空的本地 Vault；Android 能完成 Bridge 配对；三份验证结论 |
| M1 本地阅读闭环 | S1、S2 本地部分、S4 核心、S9 基础导入 | 离线导入、阅读、高亮重启后恢复、笔记自动保存 |
| M2 同步 | S3 | 桌面与平板互通笔记和高亮；并发编辑不丢数据；PDF 按需下载 |
| M3 远程 Agent | S5、S2 的 Bridge 加固 | 两个通道都能对话、审批、看历史；委派任务可离开后取回 |
| M4 手写 | S6 + 平台书写层 | 第 4 节 S6 的验收项 |
| M5 多窗口与后台完善 | S7 第 2 级、S8、S9 的系统分享入口 | 第 4 节 S7、S8 的验收项 |

M3 只依赖 M0，可以与 M1、M2 并行。M4 依赖 M1 的阅读器和 M2 的合并规则。

## 7. 测试

按仓库约定只加有实际意义的用例：

- 同步合并规则的单元测试（沿用 `engine.rs` 里的 `MemStore`）。
- 远程 Agent 客户端对上游固定向量的一致性测试；provider 契约测试覆盖不支持的能力、ID 隔离、重连去重和未知结果不重发。用一个最小测试适配器验证新增 provider 无需修改聊天 UI。
- Bridge 客户端的超时与认证顺序测试。
- 笔迹坐标变换（旋转、裁切框）的前端测试。
- 提交前按改动范围跑：`cargo test -p agentero-core`、`cargo test -p agentero export_typescript_bindings`、`cargo test -p agentero app::events_contract`、`pnpm typecheck`、`pnpm deps:check`、`pnpm test`。
- 真机矩阵和手工验收清单在平台篇。

## 8. 待决事项与风险

需要拍板的事项：

| # | 事项 | 当前假设 |
|---|---|---|
| D1（已定） | 默认远程 Agent 后端 | ds-harness-remote，保留 provider 扩展接口；服务器可配置，Bridge 为兼容适配器 |
| D2（已定） | 共享工作项分支 | `mobile`，共享变更先进入此分支，再合入 `android` 和 `ios` |
| D3 | 官方云同步要不要进首个版本（需要把账号登录移植到移动端） | 首个版本只支持 S3 / WebDAV |
| D4 | Vault 是否对系统文件应用可见 | 见平台篇，建议可见 |
| D5 | 分发渠道 | Android 沿用签名 APK；iOS 沿用 TestFlight |

风险：

- **上游协议变化快。** ds-harness-remote 逐版本跟随 DSH 适配。用固定向量测试和明确的最低主机版本来控制。
- **WebView 里的书写延迟**可能不够好，所以保留原生书写层路线，并把验证放在 M0。
- **Tauri 移动端多窗口较新**，仓库里没有先例。验证不通过时，第 2 级多窗口后置，第 1 级不受影响。
- **旧版桌面会覆盖合并结果。** 没有新合并规则的旧版本仍按修改时间覆盖 `annotations.json`。笔迹文件是新文件，旧版不会编辑，风险只在高亮。发布说明里要求桌面先升级。
- **大 PDF 的内存。** 移动端现在把整份 PDF 读成 Blob 再转 ArrayBuffer（`mobile-reader-page.tsx:68`），多窗口下每个窗口还各有一份渲染引擎。M1 要量内存并设窗口数量上限。

## 9. 参考

- 移动端现状：[../frontend/mobile.md](frontend.md)
- 同步：[../backend/sync.md](sync.md)
- 研究上下文：../backend/context.md
- Agent：../backend/agent.md
- ds-harness-remote（`11064af`，0.6.4，MIT）：<https://github.com/liguobao/ds-harness-remote>，重点是 `docs/protocol.md`、`docs/end-to-end-encryption.md`、`docs/device-pairing-v1.md`、`docs/acp-remote.md`
- DSH Go（`74e1faa`，AGPL-3.0，仅参考设计）：<https://github.com/ccch1mneyyy/dsh-Go>
- Tauri 移动端插件：<https://v2.tauri.app/develop/plugins/develop-mobile/>
