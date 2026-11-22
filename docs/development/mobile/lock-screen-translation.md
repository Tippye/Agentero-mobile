# 锁屏挂起与批量 PDF 翻译

## 现象

平板上全文翻译一篇论文（数百个版面块）时锁屏，回来看到翻译失败：块被标红，重试轮次把同一 provider 判成不可达。论文很大、翻译跑得较久时，即使不锁屏也可能出现 `renderer job report timeout`。

## 机制

1. 锁屏 = 页面进入 hidden；iOS 直接挂起整个进程（Android 也可能限制网络）。在途的 `translate_text` 请求以 DNS/超时等传输错误结束，后续请求根本没有机会发出。
2. 翻译执行器在 WebView 里跑，走 JobCenter 的 renderer offer 协议；Rust 侧 `renderer_offer_runner` 之前按**墙钟总时长** 15 分钟等终局报告。锁屏时间照样计入，于是「只是锁屏停了一会儿」被当成渲染器死亡。

## 处理

- `src/lib/core/app-availability.ts`：`appIsAvailable()`（页面可见且未报告离线）与 `waitForAppAvailability(signal)`（等 `visibilitychange` / `online` / `focus`，信号 abort 也会唤醒）。
- `src/lib/pdf/layout/layout-translate.ts` 的 `translateText`：每次请求前先等可用；锁屏导致的传输错误**不计入** provider-down 计数，恢复后重试同一 chunk。恢复时沿用既有 `layout-translate.json` 里已完成的块，只补未完成的。
- `src-tauri/src/features/jobs/mod.rs`：renderer 截止时间改为「**最近一次进度报告之后的前台空闲时间**」——进度更新即重置；`RendererClock` 记录后台累计并在恢复时冻结，后台时间整体扣除。移动端窗口的 `Suspended` / `Resumed` 同时写入 `SyncService` 与 `JobCenter`；桌面从不调用，保持原语义。

## 边界

- 锁屏期间是否继续发请求取决于平台：iOS 没有可用的通用后台联网权限，按可见性暂停；Android 在用户开启后台且通知可用时，翻译运行期间持有 transfer 租约，既有 `dataSync` 前台服务随计数启动，锁屏后继续跑（系统超时仍会结束服务，Android 15+ 约 6 小时）。租约只由 Rust 持有，JS 仅持有 id，结束后显式释放。
- 进程被系统杀掉后任务不再存活；已完成块已按批持久化在 `layout-translate.json`，重开论文仍走「补全未翻译块」，需要用户显式继续。
- 15 分钟的判定仍是安全网：渲染器真的死亡（进度不再更新）时，前台空闲 15 分钟后任务失败，不会永久占用并发槽。