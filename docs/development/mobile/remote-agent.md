# 移动端远程状态恢复

平板的远程 Agent 状态由 Rust 持有。DSH 是默认 provider；已配对用户可以选择 Bridge。Vault 文件仍是论文与笔记的权威来源，下面的设备身份和操作记录均位于 Vault 之外。

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

宿主注入存储测试覆盖身份字节与签名连续性、错误分类、未知写入、迁移期间旧记录变化、清除标记、profile CAS、并发恢复和 transport 取消。实际临时 SQLite 测试覆盖重启、取消、提交失败、并发 reservation、回执、损坏和 managed directory 丢失。UI 回归覆盖选中 provider 才恢复 Bridge、保留配对 metadata、显式重置确认、失败后重试及 WebView 丢失后恢复 native operation ID。

这些测试不替代 Android Keystore / iOS Keychain、真实 Host、设备后台及完整视觉验收；平台限制和后续工作记录在[共享实施计划](tablet-shared.md)。
