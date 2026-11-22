# 平台后台适配

平板后台由 Rust 持有任务与计数，原生层提供 OS 能力和实际状态。iOS 不提供常驻申请或伪保活令牌；远端 Agent 任务不依赖平板连接继续运行。

## 原生边界

本地插件 [agentero-mobile](../../../src-tauri/plugins/agentero-mobile/src/lib.rs) 仅在移动端注册，采用仓库锁定的 Tauri 2.12.5 公开 `PluginHandle::run_mobile_plugin_async` 和原生 Plugin 桥。WebView 不直接访问原生命令；应用的生成绑定与命令白名单负责 UI 契约。后台状态明确区分能力、用户开启、通知许可、系统前台状态和实际运行中的服务。

共享分支初始原生桥严格返回未实现、不支持的能力；Android / iOS 分支各提供自己的实际适配，不把缺失适配当作成功。内部方法为 `readState`、`configure`、`requestNotifications`、`openSettings`、`apply`；最后一个只接受 Rust 持有的 Agent / 同步 / 传输计数，不接收提示词、标题、主机信息或正文。只读查询不申请权限。

Android 使用独立的 Agent `specialUse` 与同步/传输 `dataSync` 服务；用户主动开启，新的服务只能在可见时启动，划掉任务和通知停止动作结束本机服务。iOS 仅给同步与传输提供系统允许的短时收尾窗口，Agent 计数不申请窗口。具体实施和设备边界由各平台分支的方案记录，接口见[平板共享方案](tablet-shared.md)。

## 锁屏、长任务与渲染截止时间

锁屏会隐藏页面并挂起 WebView（iOS 连整个进程挂起），在途请求只能以传输错误结束。批量 PDF 翻译因此**不追求锁屏后继续联网**，而是按请求暂停与恢复：每次 provider 请求前经 `appIsAvailable()` 等待页面可见且在线，锁屏导致的传输失败不计入 provider-down，恢复后重试同一块，已完成的块仍从 `layout-translate.json` 续跑。iOS 没有通用后台联网权限；Android 复用既有的 `dataSync` 前台服务（不新增服务类型）：批量翻译在开始/结束时经 `mobile_background_begin_transfer` / `mobile_background_end_transfer` 取得与释放 Rust 持有的 transfer 租约，原生计数进入 SYNC 泳道。租约存在且用户已开启后台（通知可用）时 `hiddenAllowed=true`，页面隐藏后仍继续发请求；否则（iOS、未开启、通知被关）按页面可见性暂停。

渲染任务的 15 分钟截止时间同样按**前台空闲**计算（[`RendererClock`](../../../src-tauri/src/features/jobs/mod.rs)）：进度报告重置空闲窗口，后台累计时间在恢复时整体扣除；移动端窗口的 `Suspended` / `Resumed` 把前台状态同时写给 `SyncService` 与 `JobCenter`，桌面端不调用、语义不变。锁屏不再被误判为渲染器死亡，而真正停止上报的渲染器仍会在前台空闲 15 分钟后失败并释放并发槽。

## Rust 任务所有权与恢复

[共享协调器](../../../src-tauri/src/features/mobile_background/service.rs)按实际工作持有 Agent / 同步 / 传输三类 RAII guard；关闭页面不结束后台所有权。释放、失败、取消、过期与 admission future 中止都会减少对应计数。进程级单调十进制 `u64` generation 防止迟到更新重新启动旧工作；计数合并后，有限窗口过期仍强制先尝试有限计数归零、再提交新工作的正计数，归零回执不能提前放行新工作。原生错误允许前台工作继续，状态始终从 OS 读取，不由计数伪造成功。

同步、按需获取和私有附件上传经 [with_work](../../../src-tauri/src/features/mobile_background/mod.rs) 接入。原生 `background-expired` 通过 Tauri 的 Rust `Channel` 直接取消同步/传输，监听确认早于 `apply`；不依赖 WebView 定时器或通知点击。中止仍以原同步状态与持久化 Unknown journal 为准，下一次前台恢复不重放发送或上传。桌面流程不经过移动窗口取消，退出时的既有最终同步保留。

iOS 的可见场景按窗口标签聚合；`Focused(true)` 同样恢复 UIKit 临时中断后的前台状态，一处 scene 退出不会暂停另一处仍活跃的 scene。进入非活动状态只停止本机 Agent 观察，有限同步/传输等待各自的完成或系统到期。原生有效 UIKit 标识符在工作仍进行时跨前后台保留，不续时、不重新申请常驻。

[会话观察器](../../../src-tauri/src/app/remote_agent/background.rs)只在用户开启 Android 常驻且发消息时登记，先取得本地工作所有权再派发。按完整 provider / account / Host / backend / workspace / session 去重，最多八个。选 Host、退出账号和 iOS 场景暂停均有代际隔离。原生每 25 秒只读补快照，读请求与恢复有界，不执行 `resume` 的默认 Host 选择、不改写回执、不重发 mutation。

队列 ACK 不证明 Agent 完成；紧随 ACK 的旧 Idle 也不证明完成。会话绑定的序列与新一轮 start/end 证据只能说明会话已静止，不能提供当前共享协议缺失的逐消息完成证明。无法证明、游标重置或连续读取失败时仅停止本机观察；Host 继续运行，操作结果仍由原 journal / 回执管理。

## 用户入口与验证边界

[后台设置](../../../src/components/mobile/mobile-background-settings.tsx)通过四个生成绑定读取状态、显式申请通知、修改 Android 偏好和打开系统设置。菜单仅在 `persistentConnection` 为真时显示；iOS 不显示常驻入口。启用前重读 OS 通知与前台状态，拒绝或通知被关闭时不调用启用。Agent / 同步运行标记只使用原生实际状态。系统设置与权限交互允许 120 秒，状态和计数调用为 5 秒；超时不自动重试。

共享分支的协调器通过 14 项工作所有权/过期回归、3 项 scene 生命周期回归、18 项会话观察/Host 隔离回归和 15 项真实公开 Dialog/Switch DOM 回归。生成绑定、事件契约、全量 TypeScript 和依赖图检查已通过。Android 平台通过 API 33 ARM64 Rust 桥检查、Kotlin 2.2.10/SDK 36 编译、10 项 JVM 测试、AAR 打包及应用清单合并；iOS 纯 Swift/Foundation 策略与 JSON 测试通过。完整 iOS SDK、APK/IPA、设备后台/网络/通知回跳与真实 Host 闭环仍需平台验证。Agent 完成/审批/提问通知、原生网络监听和系统分享不属于本阶段已完成项。

最终联合移动前端回归为 21 个文件 163 项（包含上述 15 项设置回归），全工作区严格 Clippy、只读绑定与 9 项事件契约通过。macOS 隔离 Tauri 宿主通过真实生成 IPC 验证桌面返回不支持并拒绝启用；真实 WebView 配合模拟 OS 适配器验证显式授权、双击只启用一次、非乐观运行状态、明确打开电池页面及禁用，恢复适配器后实际原生偏好仍未改变。未联系真实 Host。该宿主仍有远程渲染 AX/截屏与前台激活限制，不把程序化 DOM 验证视为可见画面、设备常驻或完整视觉验收。

## 选型

对比 [tauri-background-service](https://github.com/dardourimohamed/tauri-background-service) 的通用常驻 actor、自动恢复与单服务策略后，本项目采用系统 API 的薄适配器：需要两种独立计数服务、划掉即停止、默认不自动恢复、iOS 不承诺常驻。桥与权限组合复用 [Tauri 官方移动插件接口](https://v2.tauri.app/develop/plugins/develop-mobile/)；本地 SDK 和官方 dialog 2.7.2 的 Android/Swift 包装用于核对准确的生成和调用约定。没有复制 AGPL 的 DSH Go 实现。
