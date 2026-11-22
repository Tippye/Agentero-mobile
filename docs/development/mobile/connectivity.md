# 移动网络变化与连接恢复

网络观察属于 Rust 宿主与平台适配层，不依赖活着的 WebView。它只提示现有连接重新检查传输，不创建远程任务、配对、账号、Host 选择或后台执行额度。任务和凭据的未知结果保护仍见[远程状态恢复](remote-agent.md)，系统执行窗口见[平台后台适配](background.md)。

## 原生输入

平台分支实现 `agentero-mobile` 的 Rust 私有 `connectivityState` 与 `connectivity-changed` 频道。两者都只有 `{ "generation": "<十进制计数>" }`；计数在同一进程内单调递增，0 是初始基线，不表示断网。没有 SSID、IP、路由、网络标识或账号数据进入频道。

- Android 使用 Application 所有的单例与 Main Handler，保留弱 Plugin relay；Activity 销毁、后台设置关闭不销毁观察器。一个默认网络回调记录默认路由切换，另一个不要求 `INTERNET` 或 `VALIDATED` 的被动回调覆盖应用可访问的非默认 LAN/VPN。不请求网络、不绑定进程路由、不主动探测 Host。能力中的传输种类和 LinkProperties 只在原生内存中去重；带宽、RSSI、外网验证变化不触发重复提示。新增 `ACCESS_NETWORK_STATE` 是普通权限，无运行时弹窗。
- iOS 使用进程所有的 `NWPathMonitor`、串行队列与弱 Plugin relay。第一个真实 path 回调建立基线，随后按完整 path 等值判断变化，不把启动前的 `currentPath` 当作断网。排队的旧 owner/旧计数不会覆盖新快照；Scene 或有限后台窗口结束不销毁监视器。监视器不能延长 iOS 执行时间，也不配置新的后台模式。
- 原生计数耗尽不回绕；显式 owner 停止会隔离迟到回调，重新开始不复用计数。`mobile` 共享分支的原生叶入口继续明确拒绝尚未实现的平台能力，完整实现由 `android` / `ios` 分支提供。

网络 path 不是 Host 可达性证明。没有公网、未被系统验证的 Wi-Fi 仍可访问用户配置的私有 LAN；Rust 不用观察器状态禁止这些请求。

## Rust 接入与所有权

宿主先注册原生频道，再读取初始快照。JSON 与 Raw 频道数据经同一个显式解码器处理；未知字段、错误类型或无效消息不派发，也不关闭频道。十进制计数以字符串传输，避免 JavaScript 53 位整数边界；消费者只接收严格 `u64` 且大于当前最高值的提示。事件先于 RPC 回复、迟到旧快照和重复事件都不能倒退状态。

`app/mobile_connectivity` 使用 watch 保存最新计数，恢复进行中发生的连续变化合并为至多一个最新后续检查，不为每个回调创建任务。Bridge 与 provider 使用独立接收器，慢速 provider 关闭不会推迟 Bridge 的传输准入隔离。原生频道只持有 Rust runtime 的弱引用；退出时先取消接入与恢复循环，再关闭会话观察和 Bridge。

## 已有连接的恢复

- Provider 契约提供默认不做事的网络提示入口，允许后续 provider 自行实现。DSH 只关闭当前已连接账号所持有的 RPC 通道，保留精确 Host 引用、pins、配置、上传授权与操作回执；不调用通用 `resume`、不排序换 Host、不创建连接或读取账号列表。已有的作用域读取按同一个已批准 Host 懒重建传输。
- Bridge 只唤醒已存在的 runtime，不从配对 profile 恢复新 runtime。网络变化关闭旧传输准入并推进 epoch；RPC 在等待有界队列前记录 epoch，旧的排队请求和阻塞生产者不能越过重连边界。已派发但未收到回执的请求保留未知结果，不自动重发。未确认配对的候选连接不会因网络提示再次发送配对请求，需用户显式处理。
- 原生会话观察只唤醒仍然拥有执行额度的当前 monitor，不新建 monitor、不恢复暂停/关闭的 owner、不重置任务证据基线。网络提示触发的读取不轮换 OAuth；普通周期读取保留原有凭据预检。失败提示与连续网络抖动不消耗普通失败/停稳预算，也不能靠重复陈旧 Idle 证明任务结束。

网络提示不发送任务、审批、提问答复或附件，也不清除未知凭据刷新状态。显式断开、退出账号、切换 Host 与 Scene 暂停的所有权检查优先于恢复。

## 验证边界

平台策略使用不访问设备网络的 JVM / Foundation 测试；原生入口另做 Kotlin 编译、Swift 语法和 Android 目标构建检查。Rust 测试覆盖频道解码、计数乱序、合并与取消、队列 epoch、精确 Host、账号断开和观察预算。设备 Wi-Fi/蜂窝/VPN 切换、无外网 LAN、系统挂起与完整 iOS UIKit 集成仍需真实 SDK 和设备验收。

参考公开实现：[Now in Android 的网络观察](https://github.com/android/nowinandroid/blob/main/core/data/src/main/kotlin/com/google/samples/apps/nowinandroid/core/data/util/ConnectivityManagerNetworkMonitor.kt) 的 Application / callback 生命周期、[Alamofire 的 NWPathMonitor 组合](https://github.com/Alamofire/Alamofire/blob/master/Source/Features/OfflineRetrier.swift)，以及 [Android 网络回调](https://developer.android.com/develop/connectivity/network-ops/reading-network-state) / [Apple NWPathMonitor](https://developer.apple.com/documentation/network/nwpathmonitor) 文档。这里保留局域网能力，不复制示例中的公网验证过滤。
