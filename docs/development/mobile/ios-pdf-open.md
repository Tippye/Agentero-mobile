# iOS 系统复制 PDF 入库

本段只接收系统已复制进本应用 Documents/Inbox 的单份 PDF，复用移动端现有的本地文献库入库。它不是任意 file URL 读取接口，也不支持原提供器位置的原地编辑。iOS 分支声明 PDF Viewer、Alternate rank 和 `com.adobe.pdf`，显式关闭 `LSSupportsOpeningDocumentsInPlace` 与 `UIFileSharingEnabled`；模板、源 plist 和已跟踪生成 plist 同步，发布预检检查这些约束，不增加 Scene Manifest。

## 原生事件与深链分派

固定 Tauri 2.12.5 公开 `RunEvent::Opened { urls }`。原始事件由应用的 iOS 分支消费，不接管 AppDelegate、Scene delegate 或 WebView。Swift 插件没有公开 open-URL 回调，Tao 已将原 NSURL 转成 URL 字符串，不保留原对象的 security scope 或 openInPlace 选项，因此这里不尝试重建安全范围来读取其他目录。

固定 deep-link 2.4.9 会把 Opened 的全部 URL 发往 JS 并写入 current 缓存。iOS 使用公开 Plugin trait 包装原插件，保留全部十个公开生命周期/API hook、插件名、配置、命令、托管状态和析构，只在调用原插件前筛选 `agentero` scheme。即使筛选后为空也继续调用原插件，清除之前的配对缓存；文件 URL 不进入该 SDK 的 JS 事件或 current 缓存。原始事件不被修改，仍供私有文档入口使用。其他平台继续使用原插件。

## Inbox 打开与描述符所有权

- 只处理一份、规范 URL 表示不超过 32768 字节的 file URL。拒绝非本地 authority、query、fragment、批量和非 PDF 扩展名；解码后必须是可信原生 document_dir 的 Inbox 直接子文件。原生路径与其规范路径两个前缀用于兼容 iOS 的 var/private-var 别名。URL 库可能已规范化 localhost 与点段，不声称能恢复或拒绝规范化前的原始文本；授权检查针对实际路径。
- 规范化的根目录来自原生目录 API，不来自事件。打开 Documents 目录描述符后，通过 openat 逐级打开 Inbox 和单个文件名。目录与文件均使用 NOFOLLOW，文件额外使用 NONBLOCK；检查实际打开句柄为普通文件且 link count 为一。符号链接、硬链接、目录和 FIFO 均拒绝，不另行按源路径读取或重开。
- 原生邮箱在调度阻塞 I/O 前占用唯一槽位，Opening 和 Ready 都算忙碌。保留第一份，忙碌期间后续请求忽略，用户处理后可重新打开；当前 iOS 入口没有 Android 的原生忙碌 Toast。未完成 reservation 的 drop/panic 释放自己的槽位，完成标记用仍存活的 Arc 身份，避免旧拥有者误清后来的请求。
- 打开在阻塞工作线程执行。拒绝不相关输入会清槽；符合 Inbox 路径但不可读取的文件形成一次通用错误。就绪后只向主窗口发既有 null 唤醒事件，不含路径、URI 或文件描述符。邮箱保留 Rust File，直到当前本地文献库就绪后领取；空邮箱和打开中不会触发重复轮询。

只持有一个文件描述符，不把文件整体读入 WebView。领取后复用选择器准入锁、Vault 路径与目录身份复核、后台 transfer 租约、私有 TempDir、64KiB 分块、前 1024 字节 PDF 标头检查和严格 256MiB 单文件实际读取上限，再复用同一去重、Catalog 提交与后续准备计划。调用页面销毁不会提前释放真实操作的许可或暂存；旧页面结果不刷新新文献库。

OS 预复制发生在进入本应用之前，不受 Rust 的复制预算约束。这里不创建或修改 Inbox 原文件，也不自动清扫系统预复制件；原生内存邮箱不是持久队列，进程重启不会扫描 Inbox 或自动重放旧请求。OS 副本的存储保留/清理策略仍需另行验证。只删除自身拥有的私有暂存。

成功入库只刷新文献库，保留当前阅读、草稿与辅助面板状态。用户从文献库选择新论文阅读；没有自动 Agent 请求或远程操作重放。

## 验证与平台限制

宿主检查实际执行了 10 项文件系统/邮箱用例与 6 项公开插件适配用例：目录别名、直接子项、URI/批量边界、目录与文件符号链接、硬链接、FIFO 非阻塞拒绝、源文件 unlink 后描述符仍可读、单次领取和 reservation 放弃，以及过滤后空事件仍转交、原始事件保留和脚本 hook 委托。另有既有 10 项暂存/所有权检查通过。Mac 单元构建编译实际 iOS 算法和导入 helper 的函数体，不能据此声称完成 iPhoneOS ABI、UIKit 生命周期或设备验收。

命令签名和 null 唤醒 payload 未改变；命令文档注释更新后已重新生成并只读比对绑定，事件契约、移动白名单和严格 workspace/all-targets Clippy 通过。宿主全套 945 项中 935 通过、10 项既有外部环境用例忽略，包含绑定只读比对与事件契约。51 项既有移动端导入、配对、导出及工作区回归、类型检查与依赖图检查通过。两份 plist 已实际解析和 lint，Copy 模式、版本、配对 scheme 及项目预检通过；PDF UTI 由本机 Apple UniformTypeIdentifiers 框架核对。

当前缺少完整 iPhoneOS SDK，尚未生成 IPA。仍需设备验证：Mail/Files 等真实来源是否交付 Inbox 副本、普通冷/暖启动回调、授权/锁屏及前后台变化、重复请求、导入显示和存储清理。Tao 0.37.2 的首个 Scene 连接路径没有提取初始 URL contexts，当前应用不采用 Scene；不能把本段称为 Scene 冷启动或多窗口文档打开支持。目录外提供器 URL 不导入，用户可改用应用内文档选择器；不宣称所有 Files/iCloud 提供器均已支持。

参考：[Apple 文件系统与 Inbox](https://developer.apple.com/library/content/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/FileSystemOverview/FileSystemOverview.html)、[原地打开配置](https://developer.apple.com/documentation/bundleresources/information-property-list/lssupportsopeningdocumentsinplace)、[Tauri 公开 Plugin trait](https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri/src/plugin.rs)、[deep-link 原插件](https://github.com/tauri-apps/plugins-workspace/tree/v2/plugins/deep-link)、[Unix 锚定目录打开的标准库实现](https://rust.googlesource.com/rust/+/85eaa63195a932c731059139342153c1a55a3ddd/library/std/src/sys/fs/unix.rs)。实现复用原插件和现有入库，不 fork SDK，不引入新的渲染器文件路径权限。
