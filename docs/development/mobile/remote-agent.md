# 移动端远程状态恢复

平板的远程 Agent 状态由 Rust 持有。DSH 是默认 provider；已配对用户可以选择 Bridge。Vault 文件仍是论文与笔记的权威来源，下面的设备身份和操作记录均位于 Vault 之外。

## DSH 第三方登录

dsh-remote（ds-harness-remote）默认只提供 GitHub、Google、知乎等第三方授权，没有账号密码登录。移动端与桌面设置页共用同一套流程：`remote_agent_oauth_providers` 发现服务器提供的登录方式（404 时回退到旧的逐项 status 端点），`remote_agent_oauth_start` 创建一次浏览器授权会话，移动端用系统浏览器打开 `scanUrl`、桌面展示二维码并可选打开浏览器，随后 `remote_agent_oauth_poll` 轮询直到完成。账号 token 只在 Rust 内换取设备凭据（`auth/me` → `devices/register`），不经过 IPC；轮询会话由 Rust 按 qrId 持有并在完成/过期/取消后丢弃。发现响应的入口路径必须同源且位于 `/api/v1/auth/` 下，`scanUrl` 必须留在服务器 origin。登录成功后账号视图带 `loginProvider`，凭据记录向后兼容旧格式。桌面入口在设置 → Agent →「远程 Agent」分组，与移动端共用同一原生账号。

实现：[REST 客户端](../../../src-tauri/src/integration/dsh_remote/api.rs)、[登录命令](../../../src-tauri/src/app/remote_agent.rs)、[共享登录组件](../../../src/components/remote-agent/dsh-remote-sign-in.tsx)、[设置分组](../../../src/components/settings/panes/agent/dsh-remote-connection.tsx)。

## 主机在线状态与信任

新版 Server 的设备列表（`GET /api/v1/devices`）不再内联 `online`，在线状态改由 `GET /api/v1/devices/{id}/presence` 提供。客户端与官方 dsh-Go / app-core 一致：列出主机后逐台读取授权描述符（含 identityKey），描述符缺少 `online` 时再查 presence；查询失败只记为「状态未知」，绝不降级成离线。`HostInfo.online` 因此是可空布尔，UI 仅在明确 `false` 时禁用选择。已固定（pin）且密钥与 Server 一致的主机带 `pinned` 标记，列表显示盾牌图标；选中新主机后点击「信任并连接」即可确认。Rust 重新获取描述符，核对确认时捕获的指纹并固定该密钥。连接前 Relay 会再次核对描述符，明确离线的主机返回 `HOST_OFFLINE`。设备注册名使用 `Agentero (主机名)`，便于在其他客户端的设备列表中辨认。

桌面和移动端可以从 Agent 面板的 ACP 后端菜单选择「远程 Agent → dsh-remote」。主机下拉菜单位于后端旁边。远程和本地对话共用 Agent 面板的输入区、历史、消息、思考与工具卡片、审批、问答和 OpenUI 渲染。连接主机后，空白对话即可选择模型。主机支持 `workspace.upload.v1` 时，输入区可以添加文件，按下发送才上传。远程附件在提示词中以主机工作区相对路径传递。用户选中的论文区域也可作为图片随消息发送。

论文会话在主机用户目录下的 `Agentero/Papers` 运行。v3 主机创建的会话没有所属项目。v1 主机使用独立的 `Papers` 工作区。历史菜单也可打开此前存放在 `Papers` 工作区的会话。主机提供上下文用量和容量时，输入区显示相同的上下文占用面板。操作回执在后台保留，会话仅提示需要处理的异常。

移动端设置页另有「Agent」分区（[MobileAgentPane](../../../src/components/mobile/mobile-agent-pane.tsx)）：应用级 Agent 偏好 + 远程 Agent 连接模块；本地 CLI 安装、Node.js 工具与模型席位仍是桌面专属。

## Bridge 身份与配对

Android/iOS 的 Bridge Ed25519 身份复用应用管理的系统 `CredentialStore`，与 DSH 登录及同步凭据共用后端健康状态和串行读写通道。系统记录为 Rust 私有的 v2 envelope；设备 ID、私钥、公钥和创建时间不进入 IPC。桌面继续使用原有文件身份。

只在明确的配对动作或已保存配对的恢复过程中访问系统存储。默认 DSH 模式只读取 Bridge 的公开配对 metadata。恢复已配对主机必须加载已有签名身份；缺失、损坏、锁定或不可用均返回各自的错误，不生成替代身份。只有明确发起 Pair、且确认没有任何已有身份时，才允许创建新身份。

迁移顺序为：新系统记录 → iOS 旧 Keychain → 旧 JSON（Android 仅有 JSON）。严格验证受支持版本、成功解码的 32 字节密钥和派生公钥一致性，原始有效 JSON 字节保持不变。写入并确认新记录后，才在共用文件变更通道内比较并删除匹配的旧快照；等待存储期间改变或冲突的旧记录保留。未知写入保留同一候选值用于重试，禁止读取失败后降级或重新生成。

连接生命周期通过一个异步 owner 串行准备。旧配对 profile 保留到新 PairOk，PairOk 还要匹配当前连接代次和磁盘 profile 快照。断开会使等待中的准备和旧 transport 失效；每次 transport await 前后及每个排队 RPC 都检查 owner/stop。已经开始发送的操作仍可能已在主机执行，取消本地等待不代表撤销远端操作。

“断开连接”只停止 transport，保留签名身份与配对。显式“重置 Bridge 身份”需要确认，并先持久写入、回读验证清除标记，之后才停止 runtime 并清理旧 Keychain、JSON 和 profile。部分清理失败保留标记并返回错误；恢复不能从旧来源复活身份。清理完成后，只有下一次明确 Pair 可以生成新身份，电脑需要重新批准。此动作不自动修改桌面设备授权表。

实现：[身份存储](../../../src-tauri/src/integration/bridge/identity.rs)、[连接 owner](../../../src-tauri/src/integration/bridge/client.rs)、[命令](../../../src-tauri/src/integration/bridge/client_commands.rs)。

## 操作 journal

移动端使用应用私有目录 `app_data_dir/remote-agent/operations.sqlite`，不放入 Vault。复用现有 `rusqlite`，以 provider / connection / operation ID 为主键；保存规范化请求的 SHA256、类型化 workspace/session 目标和经过校验的结果，不保存提示词、审批答案、附件内容、本地上传路径或 provider 的原始诊断。桌面默认仍为本次启动内存 journal。

1. provider mutation 之前，先通过 FULL 事务及文件/目录同步持久提交 Unknown intent。
2. 已存在的 ID 必须匹配原请求摘要和 namespace；Unknown 或仅有导入回执的 ID 均不重新派发。
3. 发送 future 被取消、provider 返回异常，或最终结果持久化失败，都保留 Unknown。发送后发生的存储错误以 `Ok(Unknown)` 返回，避免被前端误当成确定的发送前拒绝。
4. 回执查询只观察结果。NotFound、过期、不支持或查询失败都不能证明操作没执行；已有确定结果不降级为不确定。
5. `remote_agent_local_operations` 只读本设备记录，不刷新凭据、不联系主机。WebView 重建后据此恢复操作 ID 和未知结果的目标阻塞状态；记录恢复完成前不开放新的 mutation。

数据库和行的版本、schema、namespace 均严格验证。独立的 sibling guard 识别丢失的数据库、标记或整个已管理目录，禁止重新建空历史。文件使用 owner-only 权限。存储错误只阻止新的远程写操作；lazy 初始化保留本地论文库、主机列表与历史读取能力。写入不确定后，本次 adapter 停止读取可能仅存在于缓存中的状态。

实现：[provider 契约](../../../src-tauri/src/features/remote_agent/README.md)、[journal 模型](../../../src-tauri/src/features/remote_agent/journal.rs)、[SQLite adapter](../../../src-tauri/src/integration/remote_agent_journal.rs)。

## 验证边界

宿主注入存储测试覆盖身份字节与签名连续性、错误分类、未知写入、迁移期间旧记录变化、清除标记、profile CAS、并发恢复和 transport 取消。实际临时 SQLite 测试覆盖重启、取消、提交失败、并发 reservation、回执、损坏和 managed directory 丢失。UI 回归覆盖选中 provider 才恢复 Bridge、保留配对 metadata、显式重置确认、失败后重试及 WebView 丢失后恢复 native operation ID。DSH 第三方登录另有回环 REST 测试（provider 发现与 404 回退、QR 会话 start/poll、`auth/me` 与设备注册、跨 origin scanUrl 与不安全入口路径拒绝）和前端 service 测试（发现 → 开始 → 轮询完成/过期/取消，账号 token 不经过 IPC）。

这些测试不替代 Android Keystore / iOS Keychain、真实 Host、设备后台及完整视觉验收；平台限制和后续工作记录在[共享实施计划](tablet-shared.md)。
