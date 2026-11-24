# 移动端前端与远程连接

> `mobile` 分支正在从伴侣端扩展为本地优先的平板应用，完整范围和验收状态见 [平板实施方案](tablet-shared.md)。下文 §1–§8 保留既有 Bridge 伴侣端的架构与历史决策；其中「不做本地 Vault」已被新方案取代，不能作为当前分支的产品约束。

## 当前分支的本地阅读闭环

- 启动不再强制配对。在 `appDataDir()/Vault` 创建或恢复库，文献列表读取本地 Catalog；回到前台、同步结束和手动刷新时重扫。
- 左侧导航提供文献库、广场与 Agent，沿用桌面入口与业务组件，并加入搜索、标签、论文列表、魔棒入库与论文信息；不提供写作入口。广场复用订阅源、arXiv 推荐、会议截止日期与技能浏览，外部跳转走原生 opener；移动端不执行本地技能安装或 Agent。订阅源/arXiv 的入库操作先打开魔棒供用户确认，不提前标记成功。广场页不再内置魔棒按钮。
- 广场顶部与电脑端标签页对齐为一行：左侧 `广场` 标题、来源下拉框、刷新图标、仅在来源存在公网页时显示的打开浏览器图标。打开浏览器只显示图标，经原生 opener 打开规范 URL；刷新重新挂载当前来源（内嵌网页会重新加载、原生面板会重新读取），不再有第二层“预览提示/重载”工具条。
- 魔棒支持标识、标题与候选选择。显式 `native:true` 的 lookup 入库由 Rust JobCenter 使用现有解析、下载与 Catalog 管线执行，不依赖桌面 renderer；关闭弹窗后原生任务仍可继续，完成后只从实际提交结果刷新。移动端入库参数及任务访问限定应用私有 Vault，取消与失败不回滚已正确提交的论文，重试须用户明确操作。PDF 文件仍可经私有临时目录导入；Android/iOS 暂存使用 Core 的应用私有 cache 根，不依赖 `HOME`，桌面与 CLI 保留既有目录。原生分享与文件提供器待平台实现。
- 论文信息复用桌面业务面板，保留书目信息、摘要、标签、原文及相关服务跳转、EasyScholar 指标与密钥设置。移动端隐藏桌面写作与全局编辑入口，元数据更新按当前论文身份回读，跳转仅允许经原生 opener 的 HTTP(S) 地址。
- 文献库论文行支持左滑露出操作按钮：固定（仅在该论文已有同步状态时显示，位于左侧）与删除（右侧）。固定按钮切换该论文的离线固定状态，不占用行内容宽度；删除把论文目录移入 Vault 回收站（与桌面删除语义一致），需二次确认，失败保留行与重试入口，不删除正在编辑且草稿未保存的当前论文。含固定的行展开 144px，仅删除的行保持 72px；滑动上限跟随同一宽度。移动端暂未提供回收站恢复界面，删除前确认是当前唯一恢复入口。
- 阅读器使用本地 PDF 字节与真实 `paperAbsPath`/`vaultPath`，让已有高亮与标注存储正常工作。容器按 `<600`、`600–839`、`≥840` 三档切换阅读、对照、研讨布局，复用公开的可拖分隔组件，并按预设保留比例；宽档阅读时可用标题栏的论文列表图标折叠/展开左侧列表；开关始终位于阅读器外，列表折叠后分隔条一并隐藏，阅读器获得释放的宽度。中档和窄档不挤入列表，仍由菜单打开已有导航抽屉。折叠偏好仅在当前会话保留；退出论文回到文献库时列表始终可见。复用分栏组件公开 layout 接口并分别保留展开/折叠比例，不改变阅读预设，也不打开 Agent。PDF、笔记和 Agent 的固定 pane 不随宽度、预设、tab、列表折叠或弹窗重新挂载，页码、缩放、笔迹和草稿保留。
- Android 原生内容帧避让系统栏、刘海与键盘，iOS/browser 使用四边安全区域；实际视口缩小，不用密度换算或平移伪造边距。完整约定见[安全区域](safe-area.md)。
- 可编辑的移动 PDF 在首次笔接触时进入绘制，手指平移，双指复用 SDK 缩放；笔与手指静止长按 1200ms 可选择文字、笔迹或源 PDF 区域，移动超过 6 CSS px、取消、换文档或失去活动状态会取消长按且不留下笔点。单指滑动的 touchmove 由阅读器消费，避免 WebView 原生划选文字与滚动抢手势；引文/链接的透明命中 button 也归手指平移接管，click 仅在 pointerdown 到 click 位移容差内触发（与版面元素共用 4px 守卫），起手在命中区上的滑动不再误触发跳转或 EmbedPDF 拖选。移动 runtime 的 EmbedPDF 拖选阈值提高到 10 页面单位、字形命中容差降为 1.0 倍字高，兜底只读表面上的轻滑误选。标准笔侧键/反转橡皮擦与原生工具动作经当前可见文档绑定；iOS Pencil 双击尊重系统偏好，Android 品牌专属笔身双击通过适配器扩展，不宣称通用物理连接或双击检测。设备与扩展边界见[手写笔适配](stylus.md)。
- 手写模式提供笔、荧光笔、按笔擦除、套索移动/复制/删除、撤销和重做。笔写字，手指经公开 viewport API 平移，双指复用现有缩放；书写时停用边缘返回，活动手势和待保存修改均纳入退出保护。笔未抬起停留 500 ms 会把当前笔迹吸附为规范图形（直线、箭头、矩形、三角形、圆/椭圆），继续绘制恢复手写，抬笔提交图形，机制见[PDF 笔迹](pdf-ink.md)。原生 CAS 失败保留同一笔迹 UUID，重试和丢弃均需显式操作。桌面能显示同格式笔迹。
- 设置页按电脑端结构聚合移动端全部配置：外观（主题、界面主题、语言、字号与字体）、翻译、排版解析、同步（S3/WebDAV 与下载策略）、后台运行、关于。翻译只显示现有 REST/API 服务，版面解析仅显示 Paddle/MinerU，不创建桌面设置窗口、不执行本地 Agent、ONNX/OCR 或失败回退；桌面专属栏目（账户、Agent、插件、诊断、键盘、远程访问）不进入移动设置页。侧栏只保留一个“设置”入口；宽屏为左侧分区列表，窄屏为可横向滚动的分区行。系统返回先关闭页内弹窗，再离开设置页。
- 阅读页的手动解析/翻译复用已有 JobCenter 与 PDF 叠加层；打开或导入论文只显示缓存，不自动上传。任务执行器由移动壳持有，离开当前论文不会取消全文处理；无文件 watcher 时，headless sidecar 写入通知直接更新当前阅读器。
- 缓存沿用论文 `source/layout.json`、按原文/服务/语言身份匹配的 `source/layout-translate.json` 和逐页 reflow IR；翻译只补缺失块，可显式重新翻译。IR 始终本地缓存；译文 PDF 由「翻译缓存」开关决定落论文 `translated/`（随同步）或应用缓存（仅本机），并在论文删除时清理应用缓存条目。应用重启后需用户手动继续，已完成块和已知外部解析任务均可复用。异步 API 原始结果/任务回执另存原生私有数据，文件插件不得读取或改写；未知提交结果保留并提示，不能自动重提，`force` 仅从已保存的原始结果重新处理，不新建付费任务。收据尚无自动淘汰，完整边界见[版面解析](pdf-layout-analysis.md#远程-api-的缓存与继续处理)。
- 套索可圈选无笔迹的 PDF 区域，明确点提问后进入默认远程 Agent 的参考草稿，不覆盖已有问题。用户预览并点发送后才上传截图和发送引用；未知结果保留原操作 ID，确定拒绝后的重试复用已上传附件。只有元数据的发送须明确关闭图像选项。机制及边界见[圈选区域提问](paper-context.md)。
- 阅读路由按原生窗口与 Vault 保存论文 ID、模式、工作区预设、页码和缩放。等待论文库载入后恢复；有论文的恢复默认回到阅读页并关闭研讨/对照辅助面板，新开论文或从 Agent 返回也不自动展开右侧栏。用户仍可明确打开工具、研讨、信息或圈选提问。迟到读取不覆盖用户操作，原生 CAS 冲突不自动重试覆盖。独立系统文档窗口仍待平台适配，机制见[阅读路由恢复](route.md)。
- Android 系统返回使用 Tauri 公开回调：先交给成熟 Dialog/Popover 的可取消 Escape 关闭路径，保留嵌套层次与忙碌保护，再关闭侧栏/历史、回阅读预设或文献库。笔记或笔迹未落盘时沿用退出保护；干净文献库根页面提前注销最后一个回调，恢复 SDK 与系统默认返回，不调用 `exit`、不伪造历史、不停止前台服务。原生回调注册/移除有异步交接窗口，不宣称设备级原子性；Android 停用自定义触摸滑动导航，普通滚动/笔输入仍交由原组件。迟到的独立 portal 也纳入返回保护；隐藏的 Bridge Agent 保留会话、草稿与待审批请求，但不显示对话框。相机扫描使用现有 Dialog 组合，挂载视频后独立持有 MediaStream，再交给 ZXing 公开 `decodeFromStream`；关闭时即使预览/解码尚未就绪也停止轨道，迟到流/控制器同样回收，父页面更新不会重启相机。
- 设置页的后台分区按实际常驻能力展示，通知授权、开启和系统电池页面均须用户明确操作；运行状态取自原生服务。iOS 仅给实际同步/传输登记有限窗口，不显示常驻入口。Rust 持有后台工作计数与会话观察，页面切换不重放远程操作。机制和设备边界见[平台后台适配](background.md)。
- 导出生成带标准 Ink 及压力外观的 PDF 副本，保留裁剪与旋转，不改原件。移动端复用官方原生保存对话框与文件插件，保留 SAF/security-scoped URI；拒绝在原论文目录内保存。系统分享面板仍待平台接入。格式、写入与故障边界见[PDF 笔迹](pdf-ink.md)。
- NOTES 使用成熟 Plate 编辑器与 Markdown 自动保存，保留冲突处理、未保存草稿退出保护、wiki 补全和嵌入。最后落盘也校验早期接受的精确磁盘字节；同步抢先写入时保留原始 `.md` 草稿并提示冲突。关闭时保存最新原始草稿；图片回收只在对应文档成功保存并确认引用后执行，失败或未确认时保留图片。同笔记的标题/块跳转可用，跨文档导航尚待移动工作区接入。
- Agent 页默认使用 DSH provider，支持 Host 指纹确认、工作区/会话、历史、消息、取消、审批、结构化提问和受限附件上传。Bridge 配对与聊天保留为显式可选入口。Android/iOS 的 DSH 设备身份、连接 ID、令牌和 Host 指纹经 Rust 存入系统凭据存储，桌面移动预览保持启动内会话。刷新前先保存不确定状态，回复丢失后要求重新登录；安全存储锁定、缺失、损坏与不可用分别处理，清除失败不会显示已登出。移动端任务 journal 在派发前持久保存原操作 ID 和未知状态，页面恢复先读取本地记录，未知结果不自动重发；Bridge 身份迁移与显式重置见[远程状态恢复](remote-agent.md)。
- 移动端使用独立命令白名单和应用私有文件作用域，不注册本机 Agent、CLI、SSH Vault、Word、TeX 与桌面服务命令。客户端 Bridge 命令改为 `bridge_client_*` 并导出类型绑定，避免与桌面 Host 的 `bridge_status` 冲突。
- Android 的配置/缓存/数据根在原生路径解析后、设置和身份存储初始化前指定。iOS 保留原有 Library 根，避免升级时丢失配对资料。

当前已通过移动/PDF 联合回归、TypeScript、依赖检查、事件与生成绑定契约及严格 Clippy。隔离 macOS Tauri 宿主实际验证了私有库 PDF 导入/读取、默认关闭与手动展开 Agent、信息与魔棒弹窗、广场返回当前论文及安全区域宿主；桌面无原生笔插件时返回空能力。该宿主报告页面隐藏，未验证 PDF 像素绘制；这些检查也不代表 Android/iOS 真机系统栏、键盘或笔硬件验收。最低系统版本、常驻后台、原生导入与多窗口仍以平台篇实际进度为准。iOS TestFlight 自动发布仍保持冻结。

---

## 1. 背景与决策

### 1.1 为什么改方案

原 iOS 预留方案（`src-tauri/src/app/handlers.rs` 的 `common_commands!` 集）假设 iOS 设备**本地持有 Vault**：iOS 分支注册了 `vault_tree_build`、`paper_list`、`vault_search` 等本地磁盘命令，但不含 `agent_run_once`（BYOA 需要 spawn 本机 CLI 子进程，iOS 沙箱做不到）、不含全部 `remote_*`（SSH 依赖系统 OpenSSH，iOS 没有）。

这条路线的问题：

- iOS 上没有可用的 ACP Agent（无法 spawn `claude` / `codex` 等 CLI）→ 核心能力缺失；
- 本地 Vault 需要 iCloud/文件同步方案，与 local-first 的「catalog.sqlite 单写者」冲突；
- 手机上维护第二份库，与「电脑是科研工作台」的产品定位不符。

### 1.2 新定位

**iOS = 电脑端 Agentero 的远程遥控器 + 阅读器**：

- 文件与 catalog 权威**始终在电脑**；iOS 只有缓存，没有事实来源；
- Agent 全部在电脑上运行（复用现有 ACP/BYOA 链路），iOS 只发指令、看流式输出、答权限弹窗；
- 配对与连接体验照搬 paseo：**桌面出二维码 → 手机扫码 → relay 中转 + E2EE**，任何网络环境可用，无需公网 IP / 端口转发 / VPN。

---

## 2. paseo 机制调研摘要

调研对象：**源码仓库** [`getpaseo/paseo`](https://github.com/getpaseo/paseo)（monorepo，clone 于 `~/f/paseo`，约 0.2.3）+ 本机 `@getpaseo/cli` 0.1.53 发行产物 + 本机 `~/.paseo/` 运行时数据。下文路径均相对 `~/f/paseo`。

> **许可证约束（重要）**：当前 `getpaseo/paseo-relay` 仓库为 **Apache-2.0**；Agentero 是 **MIT**（`LICENSE`）。Relay 的维护 fork 为 [`poco-ai/paseo-relay`](https://github.com/poco-ai/paseo-relay)，保留上游许可证、归属与 fork 关联。Bridge 与应用协议仍由 Agentero 独立实现，不能把 Paseo 的其他 AGPL 组件或源码带入本仓库。

### 2.1 三方架构

| 角色 | 说明 |
|---|---|
| Client | 移动 App（Expo/React Native，`packages/app`）/ Web App（`app.paseo.sh`），协议 `role=client` |
| Server（daemon） | Node 守护进程（`packages/server`），默认监听 `127.0.0.1:6767`，同时**主动出站**连 relay |
| Relay | 官方 relay 是**独立开源 Elixir 服务** [`getpaseo/paseo-relay`](https://github.com/getpaseo/paseo-relay)；monorepo 内另有 Cloudflare Workers + Durable Objects 适配（`packages/relay/src/cloudflare-adapter.ts`，按 `(版本, serverId)` 一个 DO 实例）。两者都只做 WS 帧转发，**不持有明文** |

选路（`public-docs/security.md`）：**relay 为推荐路径**（daemon 出站，无需开端口）；直连支持 TCP / Unix socket / named pipe，其中 socket/pipe 仅 CLI 可用，移动端与 Web 必须走网络。直连**不加密**，官方建议配合 Tailscale + 密码认证。

### 2.2 二维码内容

daemon 首次启动生成两个长期身份（`$PASEO_HOME`，默认 `~/.paseo/`）：

- `server-id`：`srv_` + 9 随机字节 base64url，兼作 relay 会话 ID；
- `daemon-keypair.json`：Curve25519（NaCl box）静态密钥对，`0600`。

`paseo onboard` / `paseo daemon pair` 组装 **ConnectionOfferV2**（`packages/protocol/src/connection-offer.ts:9-17`）并渲染二维码：

```
https://app.paseo.sh/#offer=base64url({
  v: 2,
  serverId: "srv_…",
  daemonPublicKeyB64: "<32B 公钥>",
  relay: { endpoint: "relay.paseo.sh:443", useTls?: true }
})
```

QR = App URL + fragment 中的 offer；解析入口 `parseConnectionOfferFromUrl`（同文件 :56）。**没有 token、没有挑战值、没有局域网 IP**——offer 是 relay-only。

### 2.3 配对与 E2EE 握手

1. App 解析 `#offer`，连 `wss://relay…/ws?serverId=…&role=client&v=2`；relay 分配 `connectionId` 并通过 daemon 的控制通道通知它；
2. daemon 为该 connectionId 建一条 `role=server&connectionId=…` 数据 WS；
3. App 生成临时 NaCl 密钥对，发**明文** `{"type":"e2ee_hello","key":<clientPub>,"capabilities":{"binaryCiphertext"?}}`；双方 X25519 ECDH 得共享密钥，daemon 回 `{"type":"e2ee_ready"}`（`packages/relay/src/encrypted-channel.ts:41-68`）；
4. 之后全部流量 XSalsa20-Poly1305 加密（`[24B nonce][密文]`，按 capability 走二进制帧或 base64 文本帧）；
5. 握手可重试：client 未收到 `e2ee_ready` 时可重发 `e2ee_hello`，daemon **重发 ready 但不换密钥**（同文件 :44-51 注释）。

**认证模型**（`public-docs/security.md` 明确承认）：daemon 在握手完成前不处理任何命令，故 relay 无法冒充；但**客户端不被认证**——「QR 码/配对链接就是密码，拿到即可连接」。轮换方式：重启 daemon 生成新会话。直连路径另有可选**密码认证**：bcrypt 存 `config.json`，HTTP 用 `Authorization: Bearer`、WS 用 subprotocol 认证，`/api/health` 豁免。

### 2.4 移动端配对实现（源码）

| 关注点 | 证据 |
|---|---|
| 扫码界面 | `packages/app/src/app/pair-scan.tsx`：`expo-camera` `CameraView`，`barcodeTypes:["qr"]`；只接受含 `#offer=` 的载荷；扫到后先 `connectToDaemon` 探活拿 hostname，再 `upsertConnectionFromOfferUrl` 落库 |
| 三种添加方式 | `packages/app/src/components/add-host-method-modal.tsx`：**直连**（手动 host:port）/ **扫码**（F-Droid 构建下隐藏）/ **粘贴配对链接**（`components/pair-link-modal.tsx`）。无 mDNS 发现 |
| 多台电脑 | `packages/app/src/types/host-connection.ts:40-48` `HostProfile{serverId,label,connections[],preferredConnectionId,...}`；一台 host 可有多条通道（`relay` / `directTcp` / `directSocket` / `directPipe`），按候选顺序回退；`orderHostsLocalFirst` 本机优先 |
| 持久化 | `packages/app/src/runtime/host-runtime.ts:1362` —— host 列表落 **AsyncStorage**（非 Keychain/SecureStore）；`utils/client-id.ts` 的 clientId 同样 AsyncStorage |
| 深链 | `packages/app/src/app/_layout.tsx:618-655` `OfferLinkListener`：App 外点配对链接可直接入库 |
| 首启 | `packages/app/src/components/welcome-screen.tsx` → `pair-scan?source=onboarding` → 成功后 `router.replace(hostRoot)` |

### 2.5 应用协议与推送

- 协议 schema 集中在 `packages/protocol/src/messages.ts`（zod，被 server/client/app 共享，另有 `generated/`）：外层 `hello`（clientId / clientType / protocolVersion / capabilities）+ `ping/pong` + `session` 包裹；内层大 discriminated union（agent 流式 timeline、权限请求/应答、文件浏览、终端 PTY、git/PR、schedule/loop…），`requestId` 关联，`clientId` 作会话键支持断线恢复。
- 传输抽象在 `packages/client/src/`：`daemon-client-websocket-transport.ts`（直连）与 `daemon-client-relay-e2ee-transport.ts`（relay + E2EE）实现同一 `daemon-client-transport-types.ts` 接口，`daemon-client.ts` 之上不感知选路。
- 推送：App 上报 Expo push token（`register_push_token`）→ daemon 持久化 `push-tokens.json` → agent 需关注且**无前台活跃客户端**时，`packages/server/src/server/push/push-service.ts` 直接 POST Expo Push API。

### 2.6 借鉴与不照搬

| 借鉴 | 不照搬 |
|---|---|
| offer-in-QR（serverId + 公钥 + relay 地址），无账号体系 | Paseo 的其他 AGPL 组件和源码；Relay fork 仅使用 Apache-2.0 授权部分，并保留上游 LICENSE 与 fork 关联 |
| relay 只转发密文、daemon 出站连接（无公网暴露面） | 客户端零认证（「QR 即密码」）：我们加**配对确认 + 设备密钥**（见 §5.3） |
| E2EE：静态 daemon 公钥 + 临时 client 密钥 ECDH，握手前不受理命令 | 直连明文 + 可选 bcrypt 密码：我们的直连也走同一套 E2EE |
| 多 host / 多通道 profile + 候选回退 + 深链配对 | host 凭据存 AsyncStorage：我们存 **iOS Keychain** |
| 双层 WS 协议、requestId 关联、断线会话恢复；transport 接口统一 | Expo 推送与 Node daemon：我们是 Tauri iOS + Rust Host（见 §8.4） |


---

## 3. 总体架构

```
┌─────────────┐   wss (E2EE 密文)   ┌──────────────┐   wss (出站)   ┌───────────────────────┐
│  iOS App    │ ◄────────────────► │    Relay      │ ◄───────────► │  桌面 Agentero (Host)  │
│  (Tauri 2,  │                    │ (Elixir/OTP,  │               │  features/bridge/      │
│   复用 src/) │                    │  仅密文转发)  │               │   ├ RPC → 现有命令面    │
└─────────────┘                    └──────────────┘               │   ├ 事件转发 agent:* 等 │
      扫码 ▲                                                       │   └ 设备配对/密钥       │
          └── 桌面 Settings → 远程访问 → 显示二维码 offer            └───────────────────────┘
```

- **桌面 Bridge**（新 feature `src-tauri/src/features/bridge/`）：桌面 App 内的连接端点，不是独立进程。开关在 Settings → 远程访问（默认关）。开启后向 relay 建立控制通道，并为每个已配对设备的连接建数据通道。
- **Relay**：**自建**（决策已定，见 §3.1）。当前采用 [`poco-ai/paseo-relay`](https://github.com/poco-ai/paseo-relay) 的 Apache-2.0 Paseo-compatible fork，部署于 `relay.agentero.app`；拓扑沿用 `serverId` 路由 + 纯密文转发，Agentero 的 Bridge/E2EE 仍独立实现。
- **iOS App**：Tauri 2 iOS 壳 + 复用现有 React 前端；不注册任何本地 Vault 命令，所有数据经 Bridge RPC。

### 3.1 Relay 服务（自建）

relay 解决的唯一问题：手机在外网、电脑在 NAT 后面，双方都无法主动连对方 —— relay 是公网**会合点**。它不存数据、不解密、无数据库；因为上层已 E2EE，relay 只见 IP / 时间 / 包大小。本质是一个按 `serverId` 配对两条 WebSocket 的交换机。

**技术选型**：采用 Elixir/OTP + Bandit + Syn 的现成实现，维护于 [`poco-ai/paseo-relay`](https://github.com/poco-ai/paseo-relay)，作为独立服务运行在 `relay.agentero.app`。这样先验证真实 WebSocket、跨节点 ownership 和部署适配器；后续如需要 Cloudflare Workers + Durable Objects，再将同一公开协议实现为另一个部署适配器，不改变 Bridge 接口。Relay 不解析上层协议，只处理 query 参数、控制消息和帧转发。

**路由与角色**（Paseo Relay v2）：

```
GET /ws?v=2&serverId=<agt_…>&role=server                       → server-control（每 serverId 唯一）
GET /ws?v=2&serverId=<agt_…>&role=server&connectionId=<conn_…> → server-data（每设备连接一条）
GET /ws?v=2&serverId=<agt_…>&role=client                       → client（relay 分配 connectionId）
```

- client 接入 → Relay 分配 `connectionId`，经 server-control 下发 `{type:"connected",connectionId}`；Bridge 建对应 server-data 通道后按 connectionId 一对一转发；
- `e2ee_hello.key` 使用标准、带 `=` 填充的 Base64（客户端发送、桌面 Host 解析保持一致）；二维码 offer 与设备身份字段仍使用 base64url。Relay 会以 `1008 Invalid handshake key` 拒绝不符合编码要求的握手；传输层保留非正常关闭的状态码与原因。
- 断开时 Relay 向对端发 `{type:"disconnected",connectionId}`；Bridge 控制通道重连后经 `{type:"sync",connectionIds:[…]}` 对账；
- Bridge 控制通道每 10s 应用层 ping，30s 无响应视为掉线；client 侧指数退避重连（1s→30s，带抖动）。

**relay 自身不做认证**（照 paseo）：`serverId` 是路由键不是秘密，安全性完全由 Bridge 侧的 E2EE + 设备验签兜底（§5.3）。relay 只做**滥用防护**：每 serverId 并发 client 上限、每 IP 建连速率限制、单帧大小上限、空闲会话回收。

**运维与自托管**：当前公网入口为 `wss://relay.agentero.app/ws`，其 `GET /health` 用于存活检查，`GET /ready` 用于就绪检查。TLS 终止层必须支持 WebSocket Upgrade 且保留 query 参数；入口层负责按 IP 限制新建连接速率与帧大小。协议与 Relay 源码开源；设置界面不展示 relay 地址（默认端点内置），自托管用户可经 `bridge_start` 的 `relayEndpoint` 参数替换。offer 里携带 `relay.endpoint`，所以换 relay 只需重新出二维码。日志只记连接元数据（serverId 前缀哈希、时长、字节数），不记内容，不记完整 IP。

---

## 4. 桌面端 Bridge

### 4.1 身份与持久化

存放在 XDG 配置目录（与 `settings.json` 同级）：

| 文件 | 内容 |
|---|---|
| `bridge/server-id` | `agt_` + 随机 base64url，长期不变 |
| `bridge/keypair.json` | X25519 静态密钥对（`{v, publicKeyB64, secretKeyB64}`，0600） |
| `bridge/devices.json` | 已配对设备列表：`{deviceId, name, devicePublicKeyB64, pairedAt, lastSeenAt, revoked}` |

Rust 侧密码学选型：`crypto_box`（X25519 + XSalsa20-Poly1305，与 NaCl box 兼容）或 `x25519-dalek` + `chacha20poly1305`；nonce 24B 前置，密文走 WS 二进制帧（不做 base64，省流量）。

### 4.2 生命周期

- Settings 开启「远程访问」→ Bridge 随桌面 App 启动/停止；桌面 App 退出即失联（iOS 端显示「电脑离线」）。
- 控制通道：`wss://<relay>/ws?v=2&serverId=…&role=server`，应用层 ping 10s 保活，断线指数退避重连。
- relay 经控制通道下发 `{type:"connected", connectionId}` → Bridge 建对应数据通道并做 E2EE 握手。
- 后续（0.8+）：可选「无界面常驻」模式复用 `agentero-cli`（`agentero bridge serve`），电脑不开 GUI 也能连——依赖 CLI 侧补 agent 能力，暂不承诺。

### 4.3 与 Vault 的绑定

Bridge 服务的是**桌面当前打开的 Vault**（多窗口时取发起开关的窗口 session）。切换 Vault 时向已连接设备广播 `vault_changed`，iOS 端清空面板并重新拉树。不做「iOS 挑选任意历史 Vault」——保持单写者与心智简单。

---

## 5. 二维码配对

### 5.1 Offer 格式

```jsonc
// QR 内容：agentero://pair#offer=base64url(JSON)
{
  "v": 1,
  "serverId": "agt_…",
  "hostPublicKeyB64": "…",         // Bridge 静态公钥
  "relay": { "endpoint": "relay.agentero.app:443" },
  "hostName": "Phil 的 MacBook Pro", // 展示用
  "pin": false                       // 预留：true 时要求确认码
}
```

- 主 scheme 用 `agentero://pair`（App 已安装场景，Universal Link 域名后置）；桌面同时提供**可复制配对链接**，iOS 侧支持「粘贴链接」与深链直接入库（照 paseo 的三入口：扫码 / 粘贴 / 手动直连）。
- 与 paseo 同构：offer 不含 token、不含局域网 IP；但拿到 offer ≠ 拿到访问权（见 5.3，与 paseo 的关键差异）。

### 5.2 UX 流程

桌面：Settings → 远程访问 → 开启 → 显示二维码 + 可复制配对链接（含「重新生成身份」按钮，等价踢掉所有设备）。

iOS 首启：

1. 欢迎页只有一个动作：「扫码连接电脑」（`NSCameraUsageDescription`）；
2. 扫码 → 解析 offer → 生成本机长期设备密钥对（存 Keychain）→ 连 relay → E2EE 握手；
3. 发送 `pair_request {deviceId, deviceName, devicePublicKeyB64}`；
4. **桌面弹确认**：「iPhone 15 Pro 请求连接，确认码 483-921」，iOS 同屏显示相同确认码，用户在桌面点允许；
5. 桌面把设备写入 `devices.json`，回 `pair_ok`；iOS 保存 `{offer, deviceKeypair}`，进入主界面。

再次启动：直接用保存的 offer + 设备密钥静默重连；Relay 断线后客户端会持续重试。连接状态变化会主动同步到移动端 UI，App 从后台回到前台时也会重新检查并触发恢复；失败时显示离线态与「重新扫码」入口。

**多台电脑**：借鉴 paseo 的 `HostProfile` 模型 —— iOS 侧保存 `hosts[]`（每台 `{serverId, label, connections[], preferredConnectionId}`，一台 host 可同时有 relay 与 LAN 两条通道，按候选顺序回退），顶部可切换当前电脑。与 paseo 不同：凭据（设备私钥 + host 公钥）存 **iOS Keychain**，不落 AsyncStorage 明文。

### 5.3 客户端认证（对 paseo 的安全修正）

paseo 不认证客户端（见 §2.3），我们补三层：

1. **配对确认**：首次连接必须桌面侧人工允许 + 双端确认码比对（防 offer 泄露后被静默配对）；
2. **设备密钥**：E2EE 握手后加一步挑战签名——Bridge 发随机 nonce，设备用长期 Ed25519 设备私钥签名，Bridge 对照 `devices.json` 中登记的公钥验签；未登记/已吊销设备只允许发 `pair_request`；
3. **设备管理**：Settings → 远程访问列出已配对设备（名称 / 最近在线），可单个吊销；「重新生成身份」全量作废。

### 5.4 无 relay 兜底

relay 不可达时（自托管用户 / 断网内网）：offer 的 `relay.endpoint` 可换成 `lan:<host>:<port>` 直连桌面 Bridge 本地监听端口，E2EE 与设备认证流程**完全相同**——这优于 paseo（其直连不加密，只有可选 bcrypt 密码，官方建议叠 Tailscale）。iOS 需 `NSLocalNetworkUsageDescription`；同样支持手动输入 `host:port`（Tailscale IP 场景）。此项为 P2，不进 MVP。

---

## 6. 应用协议（RPC over E2EE WS）

### 6.1 封装

沿用 paseo 双层结构，JSON 编码：

```jsonc
// 外层
{"type":"hello","deviceId":"…","protocolVersion":1,"appVersion":"0.7.0"}
{"type":"ping"} / {"type":"pong"}
{"type":"rpc","id":"req_42","method":"paper_list","params":{…}}       // iOS → 桌面
{"type":"rpc_result","id":"req_42","ok":true,"data":{…}}              // 桌面 → iOS
{"type":"event","name":"agent:stream","payload":{…}}                  // 桌面 → iOS 推送
```

`hello` 后桌面回 `server_info {serverId, hostName, appVersion, vault:{name, root}}`。`deviceId` 作会话键：短暂断线重连不丢 agent 订阅。

### 6.2 RPC 方法 = 现有 Tauri 命令面

Bridge 不发明新领域 API：`method` 直接映射到现有 `#[tauri::command]` 背后的领域函数（`features/paper/catalog/papers.rs`、`features/vault/tree.rs`、`features/markdown/search/mod.rs` 均以 `&Path` 为根、不依赖 Tauri State，天然可复用）。白名单制：

| 组 | 方法（首批） |
|---|---|
| Vault | `vault_tree_build` / `vault_tree_children` / `vault_read_text` / `vault_write_text` / `vault_search` |
| Catalog | `paper_list` / `paper_get` / `paper_set_tags` / `paper_set_is_read` |
| 文件 | `bridge_read_bytes`（分块拉 PDF/图片，见 6.3） |
| Agent | `agent_list_agents` / `agent_run_once` / `agent_cancel_run` / `agent_respond_permission` / `agent_list_sessions` / `agent_load_session` |
| Wiki | `wiki_backlinks` / `wiki_graph`（P1） |

**不暴露**：`remote_*`（SSH）、window/terminal/finder、Zotero connector、settings 写入、任意绝对路径读写（所有 path 参数强制 Vault 相对路径 + canonicalize 防逃逸）。

### 6.3 大文件

`bridge_read_bytes {path, offset, len}` 分块（256KB）传输，iOS 端拼装后存 App 沙箱 LRU 缓存（对齐现有 `blob_cache.rs` 语义），PDF/图片预览走本地 blob。带 `{size, mtime, sha256}` 头做缓存校验。

### 6.4 事件转发

桌面 Host 现有按窗口 emit 的事件（`events.rs`）增加一路 Bridge sink，按订阅转发给设备：

- `agent:stream` / `agent:completed` / `agent:failed` / `agent:tool` / `agent:plan` / `agent:usage` / `agent:permission-request`；
- `vault:file-changed`（驱动 iOS 端打开中的 NOTES.md 自动重载）；
- `vault_changed`（§4.3）。

---

## 7. iOS 客户端

### 7.1 复用现有前端

- Tauri 2 iOS 工程（`src-tauri/gen/apple/` 已有 `tauri ios init` 骨架），前端仍是 `src/` 的 React 代码；
- 关键抽象：在 `src/lib/core/` 加 **transport 层** —— 桌面构建下 `invoke` 直连本地命令；iOS 构建下同名调用路由到 Bridge RPC（WS 客户端可放 Rust 侧、经本地 `invoke("bridge_rpc")` 代理，密钥不出 Rust）；
- Vault handle 采用伪路径 `bridge:<serverId>`，复用现有 `isRemoteVaultHandle` 式分流经验（远端已有 `remote:<sessionId>` 先例）：跳过 fs-watch、跳过本地 wiki 索引（wiki 数据改从 RPC 拿）。

### 7.2 数据与代码边界

- **不新建手机 Vault 或 catalog**：文件、`catalog.sqlite` 和 Agent 会话仅在桌面存在；手机的读写一律经 Bridge RPC 回到桌面。
- **不新建独立前端仓库**：Library、阅读与 Agent 等业务界面继续复用 `src/` 中的 React 代码；仅新增扫码配对、主机切换、离线态和窄屏导航等 iOS 专用页面/组件。
- **手机本地只保存最小状态**：设备私钥与配对凭据存 iOS Keychain；PDF、图片和最近阅读内容可放 App 沙箱的可丢弃缓存，离线时只读，不建立写回队列。

### 7.3 界面裁剪（手机优先）

不搬桌面三栏 Dockview。iOS MVP 四个面板：

| Tab | 内容 | 复用 |
|---|---|---|
| Library | 论文列表与搜索；进入单篇论文后切换 PDF / NOTES | `paper_list`、Bridge 文件 RPC |
| Agent | 对话面板、ACP Agent 切换、历史会话、权限应答 | AI Elements |
| 侧栏 | 连接状态、当前电脑/Vault、断开、重新配对 | Bridge status |

iPad 后续可回到双栏（Library + 阅读/Agent 分屏）。

### 7.4 handlers.rs 收敛

原 `common_commands!` iOS 分支的本地 Vault 命令**移除**；iOS 目标只注册：`bridge_pair_scan`（相机结果入口）、`bridge_connect/disconnect/status`、`bridge_rpc`、`settings_get/set`（仅 App 本地偏好）、translate（可选，走免费 MT 直连）。桌面命令集不变。

### 7.5 离线行为

- 已缓存的 PDF / 最近打开的 NOTES 只读可看；
- 一切写操作与 Agent 需在线；离线时置灰并提示「电脑离线」；
- 不做离线写回队列（单写者原则，避免冲突语义）。

### 7.6 前端组件结构

移动壳位于 `src/components/mobile/`，入口 `mobile-app.tsx` 是薄容器：持有 tab 路由、配对门禁与壳布局，业务逻辑全部下沉到 hooks 与页面组件。

| 模块 | 职责 |
|---|---|
| `mobile-app.tsx` | 根容器：tab/侧栏/阅读器状态、header 装配、配对门禁 |
| `mobile-pairing.tsx` | 配对首屏：扫码（zxing）、粘贴配对链接、进度 toast、待确认验证码 |
| `mobile-agent-page.tsx` | Agent 聊天页 + 历史会话/权限弹窗 |
| `mobile-library-page.tsx` / `mobile-reader-page.tsx` | 论文列表 / PDF+NOTES 阅读页 |
| `mobile-header.tsx` / `mobile-header-actions.tsx` / `mobile-nav.tsx` / `mobile-sidebar.tsx` / `mobile-gestures.tsx` | header 壳、header 动作区（阅读模式切换、Agent 后端切换）、导航、侧栏、手势 |
| `hooks/use-bridge-status.ts` | bridge 状态订阅、启动恢复、回前台重连 |
| `hooks/use-pair-offer-links.ts` | `agentero://pair` deep-link 配对入口 |
| `hooks/use-mobile-papers.ts` | 论文列表拉取与轮询 |
| `hooks/use-mobile-agents.ts` | Agent 列表合并（注册列表 + catalog 扫描）与默认选择 |
| `hooks/use-mobile-agent-chat.ts` | 聊天状态机：事件订阅、时间线恢复、发送与权限应答 |
| `agent-sources.ts` / `chat-lines.ts` / `types.ts` | 纯逻辑（可单测）与 bridge agent 协议类型 |

约定：历史会话弹窗的开关由根容器经 props 下传（header 按钮与聊天页共享）；切换 Agent 后端时根容器清空会话 id，聊天页经 `key` 重挂载。纯逻辑模块的回归网在 `test/mobile-*.test.ts`。

---

## 8. Agent 使用（核心场景）

### 8.1 执行位置

Agent **只在桌面**运行：iOS 发 `agent_run_once` RPC → 桌面走完全现成的链路（`resolve` 默认 agent → ACP spawn → `build_prompt` envelope，含 `agentPersonalPrompt`）→ 事件经 §6.4 流回 iOS。iOS 不装、不 spawn 任何 CLI。

### 8.2 对话体验

- iOS Agent 面板：流式 markdown、Agent 后端选择、tool call 折叠、plan 展示，全部复用 AI Elements 组件；
- 上下文 chips：当前打开论文默认加入（与桌面一致）；`@` 提及数据源改走 `vault_tree_children` RPC；
- 运行中锁屏/切后台：桌面侧继续跑（这是远程执行的天然优势）；回前台经 `agent_load_session` 补齐时间线。

### 8.3 权限弹窗

`agent:permission-request` 事件转发到 iOS → 原生风格弹窗（Allow / Deny）→ `agent_respond_permission` RPC 回传。全局权限模式沿用桌面设置（`restricted/ask/auto`），iOS 只读展示当前模式、不可改（避免手机上误开 auto）。

### 8.4 通知（分期）

- **P0**：App 在前台时应用内横幅（agent 完成 / 需权限）；
- **P1**：本地通知——iOS 的 WS 后台存活受限，效果有限，明确不承诺；
- **P2**：APNs 远程推送。参考 paseo 的 attention 策略（有前台活跃客户端就不推）；需要一个推送微服务（可与 relay 同部署）持有 APNs key，推送体只含「需要你的关注」级别信息，不带正文（保持 E2EE 语义）。

---

## 9. 与现有 SSH 远程 Vault 的关系

两条远程链路**并存、不合并**：

| | `remote:` (现有) | `bridge:` (本方案) |
|---|---|---|
| 拓扑 | 桌面 Agentero → SSH → 服务器 | iOS → relay → 桌面 Agentero |
| 传输 | 系统 OpenSSH/SFTP | WS + E2EE（自实现） |
| catalog | work mirror + push-back | 无镜像，RPC 直查桌面 |
| Agent | 远端 SSH spawn | 桌面本机 spawn |
| 客户端 | macOS/Linux 桌面 | iOS |

组合场景天然成立：iOS → 桌面 → （桌面已打开 `remote:` Vault）→ 服务器。Bridge 的 RPC 打到桌面当前 Vault 的命令面即可，无需感知底层是本地盘还是 SFTP（首版可先限制为本地 Vault，验证后放开）。

## 10. 安全模型小结

- **relay 零信任**：只见密文与握手公钥；serverId 是路由键不是秘密；
- **握手前不受理命令**（与 paseo 一致）：E2EE + 设备验签未完成时，Bridge 只接受 `pair_request`；
- **offer 泄露**：对方最多能发 `pair_request`，需桌面人工允许 + 确认码（**优于 paseo 的「QR 即密码」**）；
- **设备被盗**：桌面端可吊销单个设备；「重新生成身份」全量作废；
- **命令面**：白名单 + Vault 相对路径校验，无 shell、无任意文件系统访问；Agent 的写操作仍受桌面权限模式约束；
- **密钥存放**：桌面 0600 文件；iOS 设备私钥进 Keychain（`kSecAttrAccessibleAfterFirstUnlock`），不落明文 storage。

## 11. 里程碑

| 阶段 | 内容 | 验收 |
|---|---|---|
| M0 Relay | [`poco-ai/paseo-relay`](https://github.com/poco-ai/paseo-relay)：Paseo-compatible 三角色路由与部署适配器，公网入口 `relay.agentero.app` | `GET /health`、`GET /ready` 通过；两个 WebSocket 客户端经 relay 互通；断线重连与 sync 对账通过 |
| M1 Bridge 内核 | `features/bridge/`：身份/密钥、v2 `server` 控制+数据通道、E2EE、设备配对与验签、RPC 白名单映射；Settings 开关 + 二维码 | **已完成**：已对 `wss://relay.agentero.app/ws` 完成加密双向帧联调，Bridge 单元测试覆盖协议、加密、认证与 Agent 会话过滤 |
| M2 iOS MVP | 扫码配对 + Library / 阅读（PDF+NOTES）/ Agent 对话 + 权限应答 | **功能已实现**：扫码/粘贴链接、Library 搜索、NOTES 编辑、Agent 切换、流式输出与权限应答、PDF 分块缓存、会话恢复（`agent_list_sessions` / `agent_load_session` 已入 Bridge 白名单，iOS 回前台自动补齐时间线）；下一步进入 TestFlight 内测 |
| M3 打磨 | NOTES 编辑（含保存冲突检查）、标签/已读、wiki backlinks、多主机切换、iPad 双栏 | — |
| P2 之后 | APNs 推送、LAN 直连兜底（含 Tailscale 手动地址）、headless `agentero bridge serve`、`remote:` Vault 透传 | — |

## 12. 开放问题

- Relay 域名已定为 `relay.agentero.app`；免费额度耗尽后的成本分担、定价与限额策略未定；
- 协议 schema 的单一来源：Rust 定义 + 生成 TS 类型（`ts-rs`/specta），避免手写两份；
- iOS 端 Markdown 编辑器裁剪范围（桌面 CodeMirror 栈在移动端的可用性）；
- 多设备同时在线的写并发（MVP：允许多设备连接，写入走桌面现有保存冲突检查即可）。

## 13. iOS 本地开发与构建

### 13.1 新电脑初始化

iOS 构建必须在 macOS 上进行，并需要安装 Xcode、Node.js、pnpm 和 Rust
stable。克隆仓库后执行：

```bash
pnpm install
rustup target add aarch64-apple-ios-sim
pnpm tauri ios init
```

`src-tauri/ios-project.yml`、`src-tauri/tauri.ios.conf.json` 和
`src-tauri/Info.ios.plist` 是 iOS 工程的可复现配置。`src-tauri/gen/apple/`
是 Tauri 根据模板生成的本机 Xcode 工程，不应手工维护或提交本机生成的
`libapp.a`、开发者 Team 和 scheme 噪声。

### 13.2 模拟器开发

启动指定的 iOS Simulator：

```bash
pnpm tauri ios dev "iPhone 17 Pro"
```

开发模式使用 Vite 的 `http://localhost:1420`。开发服务器必须在 App
运行期间保持启动；不要使用会在部署完成后退出并关闭 Vite 的一次性命令。
首次启动时，如果 iOS 请求本地网络权限，需要允许 Agentero 访问本地网络。

### 13.3 真机开发

真机不能访问 Mac 的 `localhost`，需要使用 Mac 的局域网地址：

```bash
pnpm tauri ios dev --host <Mac局域网IP> "<设备名称>"
```

例如：

```bash
pnpm tauri ios dev --host 192.168.1.20 "Philfan iPhone"
```

Mac 和 iPhone 必须处于可互通的网络中。真机调试还需要在 Xcode 中配置
Apple Development Team 和签名证书。

### 13.4 正式构建

正式构建会把前端打包进 App，不依赖 Vite 或 `localhost:1420`：

```bash
pnpm ios:release:check
pnpm tauri ios build
```

TestFlight 的签名、构建号和上传流程见
[iOS TestFlight Release](ios-testflight.md)。应用无账号、不需要演示登录页；
审核侧填 **Sign-in required = No**，用 **App Review Notes** 说明如何与桌面端配对
（见该文档 [Beta review: no login, no fake test page](ios-testflight.md#beta-review-no-login-no-fake-test-page)）。

## 相关文档

- 调研来源：paseo `@getpaseo/{cli,server,relay}` 发行产物（§2 有函数/文件级证据）
- ../backend/remote.md · ../backend/agent.md · ../backend/catalog.md
- release.md（iPadOS/iOS 上架清单）
