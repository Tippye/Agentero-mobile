# 移动端手写笔适配

PDF 笔迹持久化继续使用共享的 page-space ink 数据；本模块只提供能力、来源明确的笔观察和高层工具动作，不持有文件、同步或 Agent 工作。

## 当前覆盖

| 平台 / 来源 | 实际实现 | 不宣称的能力 |
|---|---|---|
| Web PointerEvent | 实际 pen 接触观察；标准按钮、反转笔尖、压力等由 PDF 输入层处理 | 不列出系统已连接的笔，不根据品牌猜测功能 |
| Android `InputManager` / `InputDevice` | 泛用适配器注册设备变化监听，以公开 `SOURCE_STYLUS` 查询输入源；标准侧键与反转笔尖仍由 Web 输入处理，避免重复切换 | 注册输入源可能是内置数字化器，并不证明某支实体笔连接；无标准笔身双击能力，无反射或未公开 OEM 接口 |
| iPad `UIPencilInteraction` | 当前可见 WKWebView 上接收双击及 Pencil Pro 挤压，分别遵循 `preferredTapAction` / `preferredSqueezeAction`，映射橡皮、上一个工具、调色板；实际收到交互后才报告观察可用 | 不枚举 Pencil 硬件连接；不假定所有 Pencil 支持双击或挤压；不输出悬停位置；完整 UIKit SDK 编译和 Pencil Pro 真机仍未验收 |
| 桌面 / 共享分支原生 stub | 返回空适配器与空原生观察，Web 输入可独立工作 | 不伪造 Android/iOS 原生能力 |

能力中的 `squeeze` 表示适配器实现了挤压路由，不表示 Pencil Pro 已连接；缺少该字段的既有适配器按 `false` 解码，Android 泛用适配器明确返回 `false`。能力中的 `hardwarePresence` 当前均为 `false`。`StylusPresence.available` 的含义必须结合 `source`：`nativeInputDevice` 是已注册的 stylus 输入源，`nativePencilInteraction` 是实际收到 Pencil 交互，`pointer` 是本视图收到 pen 指针。未知距离为显式 `proximity:null`，不是断开连接。

## 一笔成形（hold-to-shape）

活动笔迹中另有 500 ms 的一笔成形停留（`INK_SHAPE_HOLD_MS`），只把当前草稿吸附为规范图形，与静止长按选择是两条互不干扰的路径，见 [PDF 笔迹](pdf-ink.md)。

## 笔手势优先

pen 指针接触归墨迹输入层所有。笔按下、拖动、跨页和抬起期间，阅读器的跨页文本选择不接管该手势：共享选择 hook 不为 pen 设置 DOM 拖动隔离、不释放原生选区，墨迹输入在页面层消费 pen 事件。笔静止长按仍可明确进入文字/笔迹选择（复用 SDK 的字词 handler），不代表 pen 拖动可以顺带形成跨页选区；松开笔后，下一次手指拖动才继续跨页选择。桌面鼠标与触控行为不受影响。

## 数据与路由

共享插件的 Rust-only 方法是 `stylusState` 与 `stylusSetTarget`，原生事件通过 `stylus-event` 频道直接到 Rust。插件仍没有面向 WebView 的原生命令 handler；应用只开放生成绑定中的 `mobile_stylus_get_state` 与 `mobile_stylus_set_target`。

- 高层动作是 `toggleEraser`、`previousTool`、`nextColor`、`showColorPalette`、`select`。适配器的 `actions` 只列自己真正实现的子集；当前 Pencil 只产生橡皮、上个工具和调色板三种动作。`select` 只表示进入选择/套索模式，绝不代表没有坐标的文字选中。
- PDF 使用 `activateStylusTarget(documentKey)` 获得独立 UUID token，在隐藏、切文档或卸载时清理；多个订阅者必须用 `isStylusTarget(documentKey)` 确认自己仍是当前所有者。清理立即使旧 token 失效，异步原生清理不影响新文档。
- Rust 从调用的 WebView 获取窗口标签，原生激活与清理使用进程级单调递增的十进制 `u64` generation 字符串。旧 generation 不能复活旧目标或取消新目标。Rust 在任何激活之前注册原生频道，按窗口与 token 对事件做二次校验，定向发出 `mobile:stylus-action` / `mobile:stylus-presence`。
- Android 使用真实 `WebView.isAttachedToWindow`、`hasWindowFocus`、`isShown` 和 Activity 焦点检查；iOS 使用实际 WKWebView 的 key window、可见性与活动状态，存在多个无法区分的候选视图时拒绝激活。移动端不调用 pinned Tao 的通用桌面焦点 getter。桌面才使用 `is_visible` / `is_focused`。
- facade 串行提交激活/清理，等监听注册完成后再激活；收到事件时再次检查文档 token 与页面可见性。取消后的晚到监听会释放，原生不可用时不会改走未经授权的 JS/native 通道。

公开 facade：`subscribeStylusActions`、`subscribeStylusPresence`、`getMobileStylusState`、`activateStylusTarget`、`isStylusTarget`、`observeStylusPointerPresence`。公开类型来自 Rust 生成绑定，没有第二套可漂移 schema。

## 品牌扩展契约

Android `StylusAdapter` 的 `probe`、`presence`、`start`、`dispose` 和注册表是未来 OEM 适配的边界。新增品牌必须先证明 SDK/系统 API 是公开且可随应用发布的，显式加入适配器与准确的来源/能力枚举，再增加对应的生命周期与真机测试。不要根据品牌字符串开启泛用双击，也不要用反射探测私有 SDK。未实现的功能保持 `false` 或空动作列表。

iOS 的 `StylusNativeAdapter` 同样统一 probe 数据、绑定当前视图与 dispose；更换目标先移除旧 `UIPencilInteraction`，引用视图和插件均为弱引用。新硬件手势须核对最低系统、公开接口和实际硬件条件；发布验收仍需完整 SDK 类型检查与设备验证，不能因为部署下限是 18.2 就假定实际硬件具备该能力。

## Pencil Pro 挤压

iOS 分支使用 iOS 17.5 的现代 `pencilInteraction(_:didReceiveTap:)` 和 `pencilInteraction(_:didReceiveSqueeze:)`；最低系统 18.2 已覆盖可用性。移除旧的 `pencilInteractionDidTap`，避免同一双击经两套 delegate 路径重复切换工具。

- 只映射系统偏好的 `.switchEraser`、`.switchPrevious` 和 `.showColorPalette`。`.ignore`、`.showInkAttributes`、`.showContextualPalette`、`.runSystemShortcut` 和未来未知值不产生应用工具动作；不把未实现的系统面板偷换成应用调色板，也不由应用代运行系统快捷指令。
- Apple 的 `.ended` 同时用于连续手势结束和离散手势识别，因此不能强制要求先收到 `.began`。连续手势保留目标戳并在结束时复核；合法的离散结束直接按当前有效目标分派。`.changed` 不切工具；取消不提交动作。
- 挤压时间戳只检查有限、非负并用于连续重复终止回调的相等去重，不与系统 uptime 或其他时钟比较，不宣称它是全局事件 ID。
- 退到后台、绑定窗口失去 key 状态、取消或无效阶段会撤销旧交互对象并清空阶段状态。恢复只给仍获授权的同一可见视图安装新的交互对象，旧对象的迟到回调被 identity 校验拒绝；dispose 清除观察器和绑定，不能自动复活旧文档。
- 目标戳绑定 token、窗口与实际视图，不依赖会被无关旧清理推进的全局 generation 相等；现有 Rust 与前端 token 校验继续执行。没有新增手势坐标、悬停载荷或文件访问权限。

## 验证与限制

facade 测试覆盖当前 token 路由、跨文档旧清理、监听注册前取消、观察来源、隐藏视图。本次类型检查、依赖图（1477 模块、8109 边）与 57 项手写/工作区回归通过；Rust 原生频道 4 项、目标路由 4 项、生成绑定与 9 项事件契约，以及 workspace/all-targets 严格 Clippy 通过。

Swift 基线与新增 Foundation 手势策略均实际编译执行，覆盖 generation、过期清理、系统偏好、离散/连续挤压、重复结束、取消后的交互续建请求、生命周期重置、目标 token/窗口/视图变化和非法时间戳。3 份 Swift 文件语法解析通过；UIKit delegate 声明已按 Apple 公布的 Swift API 核对，但当前环境没有完整 Xcode/iPhoneOS SDK，因此不等同于 UIKit SDK 类型编译、通知时序或 Pencil Pro 真机验收。Android SDK 构建与原生检查另行执行，不能用 Swift 语法或纯策略测试替代。

参考：

- [Android InputDevice 公开 API](https://developer.android.com/reference/android/view/InputDevice) 与 [Android 13 AOSP 实现](https://github.com/aosp-mirror/platform_frameworks_base/blob/android-13.0.0_r1/core/java/android/view/InputDevice.java)。
- [UIKit UIPencilInteraction](https://developer.apple.com/documentation/uikit/uipencilinteraction)、[preferredTapAction](https://developer.apple.com/documentation/uikit/uipencilinteraction/preferredtapaction) 与 [处理双击示例](https://developer.apple.com/documentation/applepencil/handling-double-taps-from-apple-pencil)。
- [UIPencilInteraction 挤压回调](https://developer.apple.com/documentation/uikit/uipencilinteractiondelegate/pencilinteraction(_:didreceivesqueeze:))、[Phase.ended](https://developer.apple.com/documentation/uikit/uipencilinteraction/phase/ended)、[preferredSqueezeAction](https://developer.apple.com/documentation/uikit/uipencilinteraction/preferredsqueezeaction) 与 [Squeeze.timestamp](https://developer.apple.com/documentation/uikit/uipencilinteraction/squeeze/timestamp)。公开 Swift 声明以 Apple 文档数据核对，不把 Swift 语法解析当作 UIKit SDK 类型检查。
- 参考了 [flutter-apple-pencil 的现代 Tap/Squeeze delegate](https://github.com/filippofinke/flutter-apple-pencil/blob/main/ios/Classes/FlutterApplePencilPlugin.swift) 和 [munim-pencilkit 的系统偏好处理](https://github.com/munimtechnologies/munim-pencilkit/blob/main/ios/PencilKitNativeView.swift)，沿用 UIKit 公开接口，不引入这些项目的渲染或桥接框架。
- [Tauri 2.12.5 Swift 插件 API](https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri/mobile/ios-api/Sources/Tauri/Plugin/Plugin.swift) 与同版本 Android Plugin 生命周期/频道 API。
- 调研了 [Android 官方 input-samples](https://github.com/android/input-samples)，该仓库已明确弃用；不引入其旧工程或第三方输入框架。此处只封装当前公开 API，笔迹渲染仍复用现有成熟组件。
