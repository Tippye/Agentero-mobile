# 云同步（S3 / WebDAV / Git）

多设备间同步整个 Vault 到 S3 兼容对象存储（AWS S3 / R2 / MinIO / OSS / BOS 等）、任意 WebDAV 服务器（坚果云 / Nextcloud / ownCloud / NAS 等），或 Git 远端。当前已落地内容寻址同步与自动同步（状态栏指示、GC、multipart 除外），以及 Git 同步（#781；同步范围经 `.gitignore` 排除）。

引擎位于 `crates/agentero-core/src/features/sync/`（Tauri 无关），桌面端与 headless CLI 共用同一份实现——冲突处理、失败恢复与请求预算语义完全一致；CLI 侧见 `agentero sync`（cli.md）。桌面与平板在 `src-tauri/src/integration/sync/` 叠加类型 IPC、系统凭据和各自的调度；平板首版只开放 S3 / WebDAV。

S3 / WebDAV 共用同一套状态化同步协议（内容寻址 blob + 不可变 manifest + CAS `HEAD`），契约收敛在 `store.rs` 的 `RemoteStore` trait，`SyncStore` 枚举按配置选择实现，引擎对具体后端无感知。新增这类后端 = 新客户端模块实现 trait + 枚举一个变体。Git 不走 blob store：远端就是 Git 仓库本身，见「Git 后端」。设置页可切换后端；已配置的 Vault 锁定后端选择，需先解绑再切换（换后端即指向另一个远端 store）。Azure Blob、百度网盘等无 S3 / WebDAV 对应凭据模型的服务仍不能接入。

## 模块

`crates/agentero-core/src/features/sync/`（桌面端与 CLI 共享）：

| 文件 | 职责 |
|---|---|
| `config.rs` | `SyncBackendConfig`（`backend: s3 \| webdav \| git` 判别字段，旧 `sync.json` 缺省即 S3）与凭据持久化：XDG `agentero/sync.json`（按 Vault 路径分键，0600）；`secretKey` / `webdavPassword` 出站掩码 / 回传掩码保留旧值（同 translate API key 先例）；`conditionalWrites` 持久化连接测试的条件写探测结果；`webdavUrl` 归一化（trim 尾斜杠 + 坚果云根地址展开，见 WebDAV 节）；Git 的 `gitRemote` / `gitBranch`（Git 不存密钥）；`scope` 同步范围（见下；Git 把排除项写进 `.gitignore`）；`remoteIdentity()` 给出远端 store 标识（后端 + 地址/bucket/prefix + 账号名 + Git 分支，不含密钥），供本地状态在换 store 后判失效 |
| `store.rs` | 后端无关抽象（Strategy）：`RemoteStore` trait 定义后端契约（`ensure_root` / `get` / 条件 `put` / `probe_conditional_writes`，futures 均为 `Send`），`SyncStore` 枚举是唯一的组合点（从配置选择具体客户端并转发）；engine 只依赖 trait，可用内存实现做引擎测试。另含两个客户端共享的 HTTP 工具：`send_with_retries`（幂等操作传输层 3 次重试）、`check` / `etag_of` / `error_chain` |
| `s3.rs` | 最小 S3 客户端：GET / 条件 PUT（`If-Match` / `If-None-Match`）/ DELETE / ListObjectsV2，reqwest + 手写 SigV4（HMAC-SHA256 自实现，RFC 4231 向量测试）；条件写探测与降级（见下） |
| `webdav.rs` | 最小 WebDAV 客户端：Basic Auth + GET / 条件 PUT / DELETE / MKCOL / PROPFIND（仅取状态码，无 XML 解析），见下节 |
| `snapshot.rs` | Vault 扫描 → `Manifest`（relPath → sha256/size/mtime）；`size+mtime` 未变复用 base 哈希；忽略 `.agentero` `.git` `node_modules` `.DS_Store` `*.tmp`；`SyncScope` 与分类谓词（`pdf` / `source` / `attachments` / `ink` / `translated`，见「同步范围」） |
| `availability.rs` | 本机下载策略、离线固定集合、已落盘/延后条目；不改变远端 `SyncScope` 协议；按论文获取、校验后清缓存、下载状态查询 |
| `local.rs` | `.agentero/vault.json`（Vault UUID）、`.agentero/sync/{base,state}.json`（`state.json` 含 `lastError`，上次失败原因，成功即清除）、`.agentero/sync/pushed.jsonl`（已上传未发布的 blob，见「请求预算」）；watcher 忽略 `.agentero/`，无事件回环 |
| `lock.rs` | 跨进程 pass 锁：`.agentero/sync/lock` 上的 advisory 文件锁（flock / LockFileEx），桌面端、CLI 与 cron 在同一 Vault 上串行化；锁随文件句柄释放，进程崩溃不残留 |
| `engine.rs` | 三方合并 + 应用 + 发布 + 空转短路 / 断点续传（见下）；`sync_vault_locked` 在进入 pass 前取跨进程锁（冲突时报 `sync_busy` 域码）并把失败写入 `state.json` 的 `lastError`；按 backend 把 Git 分派到 `git.rs`；`test_connection` 同时接受 Vault 路径供 Git 检查仓库状态 |
| `git.rs` | Git 同步（见「Git 后端」）：系统 `git` 子进程、仓库/分支/origin 校验、`.git/info/exclude`、按 scope 写 `.gitignore` 并 `git rm --cached`、commit → fetch → merge → push、原生冲突 abort、差异计数与 catalog 刷新 |

`src-tauri/src/integration/sync/`（原生壳层）：

| 文件 | 职责 |
|---|---|
| `mod.rs` | 引擎 re-export + `SyncService` 与 `SyncRunLease`：手动、自动与退出同步共用按 Vault 的进程内占用，guard 释放覆盖正常返回、错误、超时和 task abort（跨进程占用由引擎内文件锁负责） |
| `commands.rs` | 状态、主同步源与镜像配置、解绑、同步、分类体积、下载策略、论文可用性/获取/清缓存/变更提示；移动命令校验应用私有 Vault；广播 `sync:state` / `sync:progress` / `sync:mirror` 事件 |
| `credentials.rs` | 共享系统凭据门面、目标绑定、旧配置迁移、双代引用提交与状态恢复；不向 JS 暴露通用秘密读取接口 |
| `scheduler.rs` | 自动同步：每 Vault 一个后台任务——启动时同步一次、改动静置 30s 后同步、按 `intervalMinutes`（15/30/60）定时兜底；退出时尽力推送（每 Vault 限 5s） |

## 平板、本机下载与凭据

平板同步入口复用成熟的 S3/WebDAV 设置表单，固定操作应用私有 Vault，不进入桌面官方账号或 Git 工作流。新平板首次使用时初始化「按需下载」，已有明确下载策略保持不变；桌面与 CLI 未配置策略时仍为完整下载。移动新连接默认关闭 LaTeX 源码类别，版面缓存始终保留。同步完成后先重新扫描磁盘再读取 Catalog，避免只刷新 SQLite 而看不到新下载的论文。

一个 Vault 配置一个双向主同步源，并可添加最多三个 S3/WebDAV 只读镜像。镜像沿用主同步源的同步范围，并各自保存凭据和运行状态。若范围内还有按需下载的文件未落盘，本次镜像发布会暂停，以免从镜像快照丢掉这些文件。移动端通过系统凭据服务保存镜像密码。

本机策略与可用性记录位于 `.agentero/sync/availability.json`，配套 guard 检测已管理记录丢失；结构损坏、未来版本或部分写入均报错，不退回空历史。元数据、笔记、标注、内嵌资源及 `source/layout.json` 始终同步。按需模式对尚未缓存的 PDF、源码和附件保留远端条目，缺少本地文件不构成删除。打开缺失 PDF 时去重获取该论文，再重新扫描并重试本地字节；「离线保存」更新固定集合并获取，取消固定保留本地文件。获取仍遵守已启用的类别。清缓存是显式操作，只允许移除与已确认清单相同的未编辑资源，先记录延后状态，再删除文件；没有自动清缓存。

原生类型命令包含下载策略读取/设置、论文可用性列表、按论文获取、显式清缓存及本地变更提示。平板保存 Markdown 和删除文件会通知所属 Vault；移动调度仅在前台运行，启动/恢复时执行一次，保存静置 30 秒及间隔触发；后台暂停新 pass，已进入的 pass 继续安全完成。Android 前台服务与 iOS 有限后台另行接入，不把普通调度视作系统保活。

平板密码由仅 Rust 可访问的系统凭据服务保存，公共配置只保留不含密码的参数和 `credentialRef`。保护记录按 Vault 隔离，凭据绑定结构化的后端、目标与账号摘要。原生写入成功后才发布公共引用；每次替换前先确认当前引用已持久化，再保留当前与候选两代，覆盖重命名已可见而目录刷盘失败的重试。旧明文配置迁移失败时保留原件；锁定/不可用不冒充未配置，丢失/无效时要求重新输入密码。密码掩码不能用于换目标或修复丢失。清除失败返回错误；若原生删除成功而公共删除失败，状态明确为缺失，显式重试解绑完成清理。桌面预览维持桌面存储行为，不宣称验证了设备系统密钥库。

本地基线绑定远端配置与稳定云命名空间；更换目标先持久清空基线、重置可用性历史并保留策略，最后提交新绑定。未知来源的旧基线保守重建。空转判断同时比较版本、已确认清单键与基线版本；同版本不同清单的无 CAS 分叉按保守共同祖先合并，CAS 重试一起推进祖先版本和键。已建立同步历史后 HEAD 或基线消失会报错，不能当成普通远端删除。损坏状态不会被错误记录覆盖。

## Remote 布局与一次同步

```text
<prefix>/vault.json                  { vaultId, formatVersion, encryption }
<prefix>/HEAD                        { version, manifestKey, updatedAt } ← 唯一可变对象，CAS 推进
<prefix>/manifests/<v>-<nonce>.json.gz
<prefix>/blobs/<aa>/<sha256>         gzip(内容)，内容寻址天然去重
```

一次 `sync_now`：扫描 → GET HEAD/manifest → 与本地 base（上次同步清单）三方合并 → 应用远端改动（临时文件 + rename 原子落盘，blob 校验 sha256）→ 上传新 blob（`If-None-Match: *`，跨设备重复上传为廉价 no-op——**但坚果云忽略该头**，重复 PUT 会重传全部字节，见「请求预算」）→ 发布新 manifest → `If-Match` CAS 推进 HEAD。CAS 失败（他端并发推进）则以对方 manifest 为新 base 重跑，最多 5 次。

### 请求预算（空转短路 / 断点续传）

按请求数计费或限额的后端（坚果云 WebDAV 免费版 600 次/30 分钟、付费版 1500 次/30 分钟，且为**账号级**，与官方客户端共享）下，空转 pass 与失败重试的开销才是主要成本。引擎因此在三处收敛，判据均只比 **hash**（不比 mtime，见下）：

- **已是最新**：GET HEAD 后，若 `HEAD.version == state.lastVersion`、本地扫描与 base 内容一致、且 `base.scope == cfg.scope` → 直接返回，**整个 pass 只有 1 次请求**（原先约 11 次：vault.json/HEAD/manifest 三个 GET + 每次上传前的 MKCOL 链 + blob/manifest/HEAD 三个 PUT）。此路径同时跳过 `ensure_remote_identity`——空转 pass 不写远端，无需重新核验身份；任何真正要写的 pass 仍会核验，因此远端 `vault.json` 被外部删除后最迟在下一次有改动的 pass 补回。要求 `lastSyncAt` 非空（有同步历史），首次 pass 必须走完整握手。
- **无需发布**：合并应用完成后，若 `merged` 与**未过滤的**远端 manifest 内容一致 → 把它连同当前 version 写回本地 base/state 后返回，不发布新 manifest、不推进 HEAD。比 mtime 会误判：每次下载都以新的本地 mtime 落盘，而 `merge` 在双侧 hash 相同时采纳本地条目，于是每台设备都把自己的 mtime 重新发布一遍——两台设备会**永久互相推进 HEAD**，即使无人编辑。空转短路依赖 HEAD 版本稳定，所以这一条是它能在多设备下真正生效的前提。比对用未过滤的远端清单，是为了让「对某分类失明」的设备也能区分「无可补充」与「发布端看不见我的文件」（后者仍需发布）。
- **断点续传**：上传循环跳过 `.agentero/sync/pushed.jsonl`（`local.rs`）已记录的 hash。pass 在上传途中失败（限流、退出、断网）时 HEAD 未推进，远端 manifest 仍描述旧状态，下一趟会算出**完全相同**的上传清单——没有这份日志就会把已落地的 blob 全部重传，在忽略 `If-None-Match` 的坚果云上是全额字节重传。日志**逐条追加**（一行一个 sha256），因此进程被杀或 task 被 abort 最多丢最后一条；发布成功（或走「无需发布」）即删除——此后远端 manifest 就是权威。首行以 `#<store id>` 钉住远端 store：`store_id = sha256(cfg.remote_identity())`（后端 + 地址/bucket/prefix + 账号名，**不含密钥**，故可落在 Vault 内），换 store 后旧记录一律失效，否则会发布出引用不存在 blob 的 manifest。顺带也去重了同一 pass 内内容相同的多个路径。

`MemStore`（`engine.rs` 测试内的内存 `RemoteStore`，带请求计数与可注入的 PUT 配额）覆盖这三条：`idle_pass_costs_one_request`、`idle_devices_do_not_ping_pong_head`（含「真实改动仍会发布并被对端拉取」）、`interrupted_upload_resumes_without_resending`；日志本身的追加/去重/换 store 失效在 `local.rs` 测试内。

合并规则：单侧改动直接采纳；双侧同改 `*.md` 保留 mtime 较新者、较旧者存为 `<name> (conflict <短内容哈希>).md`，重名时追加序号，不覆盖已有副本；`*.table.json` 同样保留两份，冲突副本保留完整 `.table.json` 扩展名以便在科研表格编辑器中核对。规范的 `marks/annotations.json` 与逐页 `marks/ink/pN.json` 按 ID 和逻辑版本合并，删除使用墓碑，未知或冲突载荷保留原始副本；其余 sidecar/二进制按 mtime LWW。删除 vs 修改保留修改。Git 使用自身合并与冲突流程。

标准 S3/WebDAV 对象同步在移动端默认使用 `ask`。当同一路径确有双向字节变更时，引擎在应用文件和发布远端版本前暂停本轮，把待确认的路径和两端内容哈希写入 `pending-conflicts.json`。手机或平板前台收到同步状态后显示选择，在下次启动时也会读取待办。确认仅适用于当次显示的版本。若重新检查发现版本或冲突集合已变，应用会显示新的待确认列表。结构化笔迹和批注继续按记录合并。

笔迹的本地发布还须保持[原生保存](pdf-ink.md)的持久性：规范逐页文件及完整 SHA-256 命名的原始冲突副本先 flush 临时文件，再 rename，Unix 随后 flush ink、marks 与论文目录；只有这些步骤成功，pass 才继续上传、HEAD 与 base 推进。重命名后的 flush 失败返回 `sync_ink_durability_uncertain`，保留可能已可见的新字节，不能回滚或把错误解释成原文件未变。重试时，即便相同冲突副本已经存在，也先重新确认其落盘，再替换可能持有唯一已确认笔迹的规范文件。此规则不扩展其他文件的既有同步 IO 策略。

### 落盘时序与上游安全网分层（mobile apply-before-publish）

对象存储后端的本地落盘采用 **mobile 的 apply-before-publish 时序**：`apply_local` 先在文件 mutation lane（`crate::fs::with_file_mutation`）内把下载/语义合并写入/冲突副本/删除落盘，每一步都在 lane 内用 `ensure_current` 复核扫描基线——协作编辑器的并发保存会被拒绝（`changed during sync`）而不是被静默覆盖；笔迹替换走 `write_atomic` 的 fsync-before-rename + 目录 flush。**先落盘，再上传 blob、发布 manifest、CAS HEAD**。这与上游 0.17.0 一度引入的 stage→publish→apply（先暂存、CAS 成功后再落盘）不同：mobile 是 android 的上游，落盘语义变更必须显式而非合并副作用，故 0.17.0 同步时恢复 mobile 时序（见 `engine_marks_tests`/`engine_availability_tests`/`engine_retarget_tests` 的高价值数据安全回归）。

上游安全网作为**外层**叠加，不改变 lane 内落盘时序：

- **recovery journal**（`engine/recovery.rs`）：落盘前 `save(pending)` 记录 intent（store/head/scope/落盘前条目）；`apply_local`/上传在 HEAD CAS 之前失败时 `discard`（未发布的 pass 不被当作 accepted，已落盘的本地改动下一轮重新合并）；CAS 成功后 `accept`，`record_sync` 后 `finish`。崩溃恢复时 `resume`：accepted 且远端 HEAD 匹配 intent → 用远端 manifest + 本地观测重建 base（`needs_merge`）；pending 且 HEAD 未匹配 → discard。CAS 失败/manifest 发布失败 → discard，把已落盘的 merged 作为新的本地真相、winner manifest 作为新 base 重试。
- **scope.rs**：嵌套集合论文根识别 + 逐类对称过滤（含 mobile 的 `ink`/`translated`），排除路径双向失明、绝不因缺失被当作删除。
- **条件写**：pass 前探测远端。标准同步要求条件写。WebDAV 可显式启用上游的锁保护降级模式，移动端仍执行逐文件冲突确认。已注册旧 WebDAV 日志协议的 Vault 沿用其协议。镜像发布要求条件写。
- **fork 保守合并**：plain-PUT 后端同版本号不同 manifest（或回滚 HEAD）视为 fork，无共享祖先时按新增/编辑合并而非推断删除。

下次上游同步若再引入 stage→publish→apply，须显式评估并保留 mobile 的 lane 内落盘与上述数据安全断言，不得作为合并副作用翻回时序。

## 同步范围（Sync Scope）

论文库中体积大或由某类设备产生的附件可以按设备排除，节省云端空间；笔记、`metadata.json` sidecar、高亮/文字批注、`assets/` 永远同步（小且不可再生）。S3 / WebDAV 在清单合并时对排除路径双向失明。Git 使用同一套开关，但做法是在仓库 `.gitignore` 里维护一段 `# agentero-sync-start` / `# agentero-sync-end` 标记区块，并 `git rm --cached` 取消跟踪（工作区文件保留）。规则随仓库同步，所以是整库一份策略，不是每台设备各看各的。

- **分类**（`snapshot.rs` `scope_category`，仅识别约定论文布局）：
  - `pdf` — `papers/<id>/<id>.pdf`（论文根级 PDF；`source/`、`attachments/` 内的 PDF 跟随所在分类）
  - `source` — `papers/<id>/source/`（LaTeX / e-print）
  - `attachments` — `papers/<id>/attachments/`（支撑材料）
  - `ink` — `papers/<id>/marks/ink/`（手写笔迹 sidecar；**仅移动端设置页展示**，桌面只读显示、不产生笔迹，因此桌面始终按开启处理）
  - `translated` — `papers/<id>/translated/`（全文翻译完成后的译文 PDF；所有端都可配置）
  - 例外：`source/layout.json` 与 `source/layout-translate.json` 属于共享元数据，始终同步——译文 sidecar 是另一台设备复用已同步译文 PDF 的内容键输入，不能随 `source` 关闭。
- **对称过滤**：同一谓词同时作用于本地扫描、base 与远端 manifest——被过滤的文件「双向失明」：不上传、不下载，也**绝不因缺失而被当作删除**。
- **manifest 携带 scope**：发布端把自己过滤掉的分类写进 manifest（`scope` 字段，全量同步时省略）。合并时：远端失明的路径不触发 `delete_local`，本地仍可见该分类时以本地条目为准并继续上传，否则携带 base 条目供其他设备可见；本地失明的路径完全惰性（不下载、不传播删除）。
- **边界**：所有设备都过滤某分类时，该分类条目会从 manifest 消失（blob 仍在，待孤儿 GC）；重新启用后本地仍有文件则自动重新上传，本地没有则需从来源重取。
- **重取**：PDF/TeX 可从 `metadata.json` 的来源字段（arXiv ID / DOI / `pdf_url`）重新下载——`job_download_assets_enqueue`（DownloadAssets 后台任务；库表格右键「从来源下载 PDF」、打开论文时自动补下均走此路径）。库列表的 `has_pdf` 由 `paper_list` 经 CapsCache 投影。
- **配置**：`SyncBackendConfig.scope`（缺省值：`pdf` / `source` / `attachments` / `ink` 开，`translated` 关；旧配置缺字段按此默认，旧 manifest 缺 `scope` 字段仍按「全量」读取）；设置页以「同步范围」小标题分组展示逐类开关（附本地体积 `sync_scope_sizes`）。`translated` 只在「设置 → 翻译 → 缓存 → 缓存译文 PDF」开启并实际完成整篇翻译后才有文件；该同步开关只决定是否上传，不负责触发翻译。
- **译文 PDF 落盘**：本地论文的最终译文 PDF 写入 `papers/<id>/translated/`（按 PDF 哈希 + 内容指纹命名），另一台设备命中同一内容键时直接复用，不再重新提取与构建；远程 Vault / 无论文路径的会话回退到应用缓存。`translated` 关闭时文件仍留在本地论文目录，只是不上传、不下载。清除「设置 → 关于 → 翻译缓存」会删除本地 `translated/`，若该分类开启，删除会在下一次同步传播到其它设备。
- **笔迹与论文删除**：远端整篇论文删除的证明使用 `is_all_for_paper_presence()`（忽略 `translated`），因此默认关闭译文 PDF 不会让旧的整篇删除逻辑失效；发布端关掉 `ink` 时，笔迹按「失明」处理，不因缺失被删除。

## WebDAV 后端

`webdav.rs` 用 reqwest 手写（不引第三方 WebDAV 库），Basic Auth（坚果云应用密码 / Nextcloud 应用令牌 / NAS 账号均兼容）：

- **目录模型**：WebDAV 需显式建目录。PUT 前按需逐级 `MKCOL`（405 = 已存在），已建目录在客户端内缓存，稳态零额外请求；连接测试 `PROPFIND Depth:0` 根目录，404 则连目录一起创建——用户可直接指向一个不存在的目录（如 `https://dav.jianguoyun.com/dav/agentero/`）。坚果云根地址（`https://dav.jianguoyun.com/dav/`）自动展开为专用文件夹 `agentero/`：根集合可列表但拒绝创建文件（PUT 一律 404 ObjectNotFound），而它恰是官方文档给出的地址；展开是纯函数（trim 尾斜杠 + 根地址映射），`normalized()` 与客户端构造共用——新配置回显实际地址，旧配置无需重存即自愈。
- **连接测试**：PROPFIND 207 / 创建成功即凭据与目录可用；401/403 报凭证错误，不保存配置。随后做一次**无条件试写**（一次性 key，写完即删）：地址写不进去（不可写位置、权限或配额拒绝）在配置阶段就报错并带上目标目录 URL，不再静默保存坏配置、把错误推迟到首次同步。
- **条件写**：WebDAV 实现差异大，探测针对 `If-Match`（HEAD CAS 的唯一依托）：无条件试写建出一次性 key 后，带过期 etag 再 PUT，412 = 真支持，2xx = 降级为普通 PUT（与 OSS 共用 `conditionalWrites=false` 与降级语义，见上节）；`If-None-Match: *` 不探测——被忽略也无害（blob 内容寻址、manifest key 唯一随机）。实测坚果云忽略 `If-None-Match` 但强制 `If-Match`，即 CAS 原子性完整保留。硬错误（非 2xx/412）直接让连接测试失败；只有探测无结论（如条件头被忽略/拒绝）才按支持处理（fail open——被忽略的头无害，漏掉 CAS 才有害）。`test_connection` 以 `conditionalWrites=true` 构造探测客户端，保证测的是服务器行为而非持久化的旧结论。
- **无 ETag 的服务器**：`If-Match` 退化为普通 PUT，同降级语义收敛。
- **重试**：与 S3 客户端一致的传输层 3 次重试（幂等操作）。三次都失败且请求从未到达服务器（DNS 解析失败、连接被拒绝/重置、连接超时）时，错误域码为 `sync_unreachable`；证书被拒不算——它每次都会以同样方式失败，需要用户改证书或地址。自动同步只对 `sync_unreachable` 退避重试（见「自动同步」）。
- **安全约束同 S3**：`https://` 强制（仅 loopback 放行 http），密码掩码同 `secretKey`。NAS 场景的明文 http 与自签名证书 https 均不可用。
- **兼容性（实测）**：路径段 percent-encode、请求带尾斜杠集合形式。坚果云特性：MKCOL 对根 `/dav` 返回 403 OperationNotAllowed（按「已存在/受保护」容忍）；根 `/dav` 不允许创建文件（PUT 一律 404 ObjectNotFound，[#681](https://github.com/poco-ai/Agentero/issues/681)），故根地址自动落到 `agentero/` 子文件夹（见「目录模型」）；顶层目录名长度有限制（`sandbox name is too long`）；顶层目录不可经 WebDAV 删除、非空目录删除需先清空子项——`sync_disconnect` 本就不动远端数据，仅测试清理需注意；**请求按账号限频**（免费 600 次/30 分钟、付费 1500 次/30 分钟），空转成本见「空转短路」。Nextcloud / Apache mod_dav 按标准实现工作。
- **集成测试**：`engine.rs` `two_device_roundtrip_against_webdav`（`#[ignore]`，env `AGENTERO_SYNC_WEBDAV_TEST_URL` / `_USERNAME` / `_PASSWORD`，连接测试 + 发布/加入/编辑/分歧冲突/收敛全流程，镜像 MinIO 测试）。

## Git 后端

- **形态**：Vault 根目录就是 Git work tree；远端名固定 `origin`，分支由 `gitBranch` 指定（默认 `main`）。不经过 blob/manifest 引擎，Git 自己保存历史与合并。
- **要求**：本机安装 Git 并可通过 PATH 找到（桌面 GUI 也走 `resolve_command` 的补充路径）。Vault 不能位于另一个 work tree 内、不能是 bare repo / detached HEAD；已有仓库的当前分支必须与配置一致、已有 `origin` 必须与配置 URL 一致，否则同步报错而不静默改仓库。
- **忽略**：`.agentero/`、`.DS_Store`、`*.tmp`、`node_modules/` 通过 `.git/info/exclude` 排除，不写进 `.gitignore`；本地或远端已跟踪 `.agentero` 直接报 `git_agentero_tracked`。`catalog.sqlite` 仍可由 sidecar 重建。同步范围关闭的 PDF / `source/` / `attachments/` / `marks/ink/` / `translated/` 写在 `.gitignore` 的标记区块里；**排除项只对新文件生效，已跟踪文件继续在各设备之间同步**，关闭类别不产生删除提交。关闭 `source` 时按内容忽略（`source/*`）并显式恢复 `!…/source/layout.json` 与 `!…/source/layout-translate.json`，避免整目录忽略把始终同步的版面/译文 sidecar 一并挡掉。分类规则由 `scope.rs` 提供，支持 `papers/` 下嵌套集合的论文根识别。
- **一次 pass**：按 scope 更新 `.gitignore` → 暂存并提交范围内改动 → 查询远端分支并按需 fetch → 合并 → push。远端无分支时直接推送；本地没有提交时用 `checkout --no-overwrite-ignore -B` 采纳远端，无共同祖先时走 Git 三方合并。检出或合并前先检查远端路径是否与本地未跟踪且被忽略的文件重叠，重叠则停止请先备份移开。push 被非 fast-forward 拒绝时重新获取和合并（最多 3 次 push）。
- **冲突**：使用 Git 原生三方合并；有冲突时收集 `--diff-filter=U` 路径、`git merge --abort` 并返回 `sync_conflict`，**不 push、不 auto-resolve**。本地提交保留，用户用 Git/编辑器解决后再同步。
- **认证**：不把 token 写入 `sync.json`；使用系统 credential helper / SSH agent。子进程设置 `GIT_TERMINAL_PROMPT=0`、`GCM_INTERACTIVE=never`、`SSH_ASKPASS_REQUIRE=never`（Unix 未自定义 `GIT_SSH_COMMAND` 时加 `ssh -o BatchMode=yes`），非交互环境失败而不是挂起。HTTPS 没有凭据时设置页按 `git_auth_failed` 提示先用 `git credential approve` 或系统凭据助手存 token，SSH 则提示 `ssh-add`。URL 内嵌凭据、`ext::`、`git://` 一律拒绝。非 loopback 的明文 `http://` 默认关闭，设置页「允许明文 HTTP」（`gitAllowInsecureHttp`，CLI `--git-allow-insecure-http true`）打开后才保存，令牌以明文传输，只适合没有 HTTPS 的内网。自签 HTTPS 可填 CA 证书的绝对 PEM 路径（`gitSslCaInfo`，CLI `--git-ssl-ca`），每次 Git 命令带 `-c http.sslCAInfo=<path>`；留空用系统信任库。证书校验失败为 `git_tls_failed`。不提供关闭证书校验的开关。
- **Windows 进程回收**：每次 Git 命令经 `process::command_output` 先暂停创建、加入独立 Job Object 后恢复运行，避免 Git 启动器派生的 HTTPS / SSH 后代逃逸。命令正常结束、超时（网络命令 600s）或同步任务取消时均关闭 Job，终止该次命令留下的完整进程树；不按进程名清理，不影响其它仓库的 Git。同一守卫也用于写作目录同步、Doctor / 安装器、编译工具和 MCP tunnel。非 Windows 保持原有执行方式。全局 `http.postBuffer` 等用户 Git 配置仍由 Git 自身读取，本修复不会修改；过大的上传缓冲可能放大单次 HTTPS 推送内存占用。
- **触发**：复用自动同步调度（打开 Vault、静置 30s、定时间隔）。退出 flush 跳过 Git——5s 预算可能中断 commit/push 并留下 `index.lock`，下次启动补推。`.git` 与 `.agentero` 已被 watcher 忽略，不会事件回环。
- **限制**：关掉的类别不再进入新提交；已经在 Git 历史里的 PDF/TeX 不会被清掉（未接 Git LFS）。没有按论文按需拉取；partial clone + sparse-checkout 尚未接入。

## 条件写降级（OSS 等后端）

阿里云 OSS 的 PutObject 不支持任何条件请求头（`If-Match` / `If-None-Match` 等，带则返回 `400 NotImplemented`），S3 / R2 / MinIO 均支持。处理：

- **连接测试探测**：`sync_configure` 在 ListObjects 之后用一次性 key（`.sync-probe-<uuid>`）带 `If-None-Match: *` 试写并删除；`400 NotImplemented` → `conditionalWrites=false` 持久化到 `sync.json`，探测无结论时按支持处理（fail open）。
- **命令面**：当前没有独立 `sync_probe`；配置保存即执行连接探测，失败则不保存，成功后设置页按已连接展示。
- **运行时兜底**：旧配置未探测过时，首个条件 PUT 收到 `400 NotImplemented` 即在客户端内标记并立即以无条件 PUT 重试，同一 pass 内后续写入全部降级。
- **降级语义**：blobs / manifests 内容寻址或 key 唯一，无条件 PUT 幂等无害；HEAD 指针退化为 GET → PUT，牺牲严格 CAS，靠三方合并与重试收敛（单用户场景最终一致）。设置页对 `conditionalWrites=false` 显示一行小字提示。

## 身份与 Catalog 联动

- `vault.json`（远端）与 `.agentero/vault.json`（本地）配对：从未同步过的 Vault 可加入既有 remote（采纳其 id）；有同步历史的 Vault 拒绝外来 remote。
- 论文权威字段已 sidecar 化：每次 `upsert_paper` 同步投影到 `papers/<id>/metadata.json`；`paper_rescan` 优先从 sidecar 恢复（sidecar 较新则回灌 DB）。因此同步只处理普通文件，`catalog.sqlite` 不出 Vault；拉取后引擎自动 `rebuild_from_disk` + `prune_missing`。

## 前端

设置窗口「同步」pane：`src/components/settings/panes/sync-pane.tsx`；命令封装 `src/lib/sync/api.ts`。仅本地 Vault 可配置（`remote:` 句柄显示提示）。标题旁用小色点展示连接状态（灰=未连接，绿=已连接，蓝=同步中，红=最近一次同步/连接失败）。后端选择为与服务商 logo 同行的下拉框（对象存储 / WebDAV / Git；已配置时禁用并提示先解绑），随后按后端渲染表单（S3：endpoint/bucket/AK/SK + 高级项；WebDAV：服务器地址/用户名/密码；Git：仓库地址/分支、明文 HTTP 开关、CA 证书路径，并说明 HTTPS 需预先存好凭据）。服务商 logo 按钮按后端分组（S3：AWS/R2/MinIO/OSS/BOS；WebDAV：坚果云/Nextcloud；Git：GitHub/GitLab），打开官方配置指南，不会通过外链自动创建或回填凭据。同步范围（见上）在同一 pane：小标题 + 逐类开关（行内显示本地体积）。`ink` 仅在移动端（`mobile`）渲染；`translated` 全端渲染并附带说明其依赖翻译缓存开关。Git 下说明改为写入 `.gitignore`。

## 自动同步

配置项 `autoSync`（默认开）与 `intervalMinutes`（15/30/60，默认 30）随凭据存 `sync.json`。调度任务在 `sync_configure` 后（重新）启动、`sync_disconnect` 时停止、应用启动时按配置恢复；每次触发都重读凭据，改配置无需重启。触发器：调度启动即同步一次（≈打开 Vault）、Vault 改动静置 30s、定时间隔兜底；`RunEvent::Exit` 时对所有自动同步 Vault 尽力推送（超时 5s/Vault）。

传输层失败（`sync_unreachable`：DDNS 名称短时消失、NAS 休眠/重启、Wi-Fi 切换）不等到下一个间隔：自动同步按 30s → 2min → 10min 退避重试，成功即复位，10 分钟封顶避免长期离线时持续打远端。凭证、配置、HTTP 状态类错误不重试，仍按间隔等待用户处理。设置页对这类失败用可操作文案提示（「无法连接同步服务器…，同步会在后台重试」），原始 reqwest 错误链作为副行保留，便于排查。

同一 Vault 的手动、自动、退出同步共用 `SyncRunLease` 占用；不同 Vault 可独立运行。退出 flush 若该 Vault 已在同步则记录并跳过，避免并发改写本地 base/state。正常成功/失败先释放占用再广播既有终态，超时或 task abort 通过 guard 自动释放，后续同步可重新取得占用。

本轮只收敛占用生命周期：abort 不新增 `sync:state` 终态事件，事件驱动的 UI 可能保持旧状态直到刷新或后续事件（`sync_get_status` 已能读取正确的 `running`）。中止 future 也不保证已启动的 `spawn_blocking` 或远端请求被撤销；协调取消、等待子任务与取消后的 UI 对账仍属架构计划 E2。

## 并发（跨进程）

远端一致性由 CAS `HEAD` 保证（多设备天然串行化）；同一台设备上桌面端与 CLI（或 cron）同时跑 pass 的**本地**写入交错，由 `engine::sync_vault_locked` 入口处的 `.agentero/sync/lock` advisory 文件锁串行化：

- 拿不到锁立即失败，域码 `sync_busy`（CLI 退出码 1，桌面端记入 `sync:state` error）；使用 `fs2` 的平台锁冲突错误码识别占用，避免 Windows 的 `ERROR_LOCK_VIOLATION` 被误报为普通 IO 错误。
- 正常释放 guard 时显式解锁，再关闭文件句柄，避免复制的句柄（包括 Unix 并发 `fork` / `dup` 继承的描述符）延迟释放同一把锁。进程退出、崩溃或被杀时仍由系统在最后一个相关句柄关闭后释放，无陈旧锁文件清理。
- 桌面端手动 / 自动 / 退出同步与 CLI `sync now` 均走该入口；`SyncRunLease` 仅叠加进程内 UI 状态（`running`）。
- 锁只覆盖 pass 本身；`catalog.sqlite` 并发访问由 WAL + busy_timeout 兜底（CLI 与桌面端共用 `with_catalog` 的连接缓存语义）。

## CLI（headless）

`agentero sync`（cli.md）面向无桌面环境的服务器：同一引擎、同一 `sync.json` 绑定、同一冲突与断点续传语义。典型服务器侧接入（vault 从未同步过，引擎自动采纳远端 `vault.json` 的 id 完成加入）：

```bash
agentero vault create ~/papers
AGENTERO_SYNC_ACCESS_KEY=… AGENTERO_SYNC_SECRET_KEY=… \
  agentero sync configure --vault ~/papers \
    --backend s3 --endpoint https://<account>.r2.cloudflarestorage.com --bucket papers
agentero sync now --vault ~/papers --json   # cron / 助理调用，退出码即成败
agentero sync status --vault ~/papers --json  # lastError 给出上次失败原因

# Git：Vault 即 work tree；认证走系统 credential helper / SSH agent
agentero sync configure --vault ~/papers \
  --backend git --git-remote git@github.com:you/papers.git --git-branch main
# 内网明文 HTTP（令牌明文传输）与自签 HTTPS：
# agentero sync configure --vault ~/papers --backend git \
#   --git-remote http://git.lan/you/papers.git --git-allow-insecure-http true
# agentero sync configure --vault ~/papers --backend git \
#   --git-remote https://git.lan/you/papers.git --git-ssl-ca /path/to/ca.pem
agentero sync now --vault ~/papers --json
```

凭据安全：secret 不进 argv（flag 只收 endpoint/bucket 等非敏感字段），来源为 `AGENTERO_SYNC_*` 环境变量、TTY 交互（`--json` 下绝不提示）或 `--stdin` JSON；所有输出经 `masked()` 掩码，`remoteIdentity()` 与日志均不含密钥。Git 不接收 secret flag，也不做 TTY 提示；使用系统 Git 凭据。`--exclude` 对三种后端通用；Git 把排除项写进仓库 `.gitignore`。

## 安全约束

远端对象视为不可信输入，引擎在应用前统一校验：

- **manifest 路径净化**（`engine.rs` `validate_manifest`）：relPath 必须非空、非绝对、仅 `/` 分隔、无空段 / `.` / `..`，否则整个 pass 失败——杜绝经 `vault.join` 越界写/删文件。
- **hash 校验**：manifest 中 hash 必须是 64 位小写 hex（sha256），防止畸形 key  panic 或索引到 `blobs/` 之外。
- **解压限流**：blob 解压上限为 manifest 声明 size + 1MiB（sha256 校验兜底），manifest 解压上限 256MiB，防 gzip bomb。
- **强制 TLS**：`validate()` 要求 S3 endpoint 与 WebDAV URL 均为 https；仅 loopback（`localhost` / `127.0.0.1` / `::1`）放行 http（本地 MinIO 测试场景），避免 SigV4 / Basic 凭据明文传输。Git 的非 loopback `http://` 另有显式开关，默认关闭；自签 HTTPS 只接受用户指定的 CA 文件，不关闭校验。WebDAV 仍不接受明文 http 与自签证书。

## 边界（后续分期）

状态栏指示、孤儿 blob GC、E2EE、官方托管凭据 provider 均未实现，见设计草稿分期表。Git 后端的 LFS、按论文按需拉取（partial clone / sparse-checkout 或 `sync_fetch_papers`）同样属于后续 Phase（#781 暂不关闭）。同步范围已通过 `.gitignore` 排除，不清理已有历史。
