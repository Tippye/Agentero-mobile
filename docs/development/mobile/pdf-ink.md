# PDF 笔迹与原生编辑

笔迹保存在 `papers/<folder>/marks/ink/p<一基页码>.json`，源 PDF 保持不变；移动端设置可以关闭 `marks/ink/` 的云同步（桌面只读显示，始终按开启处理）。格式与按 ID 合并位于 [ink.rs](../../../crates/agentero-core/src/features/pdf/ink.rs)；文件存取位于 [ink_store.rs](../../../crates/agentero-core/src/features/pdf/ink_store.rs)，原生命令壳位于[原生 ink.rs](../../../src-tauri/src/features/pdf/ink.rs)。三者由桌面和平板共享，桌面阅读器当前提供只读显示和副本导出。

笔迹只相对于空白页面几何，与页面文字内容无关：原文 PDF 与译文 PDF 页面尺寸一致，笔迹在两者上按同一页坐标原位置显示。切换原文 / 译文（覆盖模式）不隐藏、不移动笔迹，也不中断书写；译文覆盖层之上仍可落笔、擦除、套索移动。双栏对照时原文与译文栏共享同一个 `InkEditor` / `InkController`（按论文 key），任一侧书写、移动、撤销立即更新另一侧并写回同一页文件；译文栏页面壳带 `data-annotation-pdf/page` 属性，笔输入桥据此把笔势映射到译文表面。原文工具栏是唯一的笔工具控制。

## 格式与坐标

版本为 1。每页包含 `strokes` 和 `tombstones`；缺失一笔不代表删除。笔迹有 UUID、页码、工具、RGBA、基准宽度、点序列、Lamport revision 和创建/更新时间。坐标与宽度使用未旋转的可见 CropBox 左上角、向右/向下的 PDF 点空间；原始 `/Rotate` 由现有页面框架处理，不能存屏幕坐标。压力为 `[0,1]` 或 `null`，零压力与设备未提供压力不同。点时间是笔内经过时间，倾角成对出现。

逻辑版本由 Rust 基于当前文件已观察的最大 counter 递增，actor 是配置根 `ink/replica-id` 的非空 UUID。actor 通过无覆盖发布、文件与父目录 flush 保持跨窗口/重启稳定；目录 flush 仅覆盖 `ink/` 与应用配置根，不向上遍历应用沙箱外的祖先。Android 的 Tauri 配置根是应用 dataDir，继续打开其父级 `/data/user/0` 会因权限失败而阻止笔迹保存，即使身份文件已经发布也会在重载时再次失败。配置根、最终目录和文件均须位于 Vault 外。已有损坏记录、符号链接、类型错误或存储失败都不会被随机新 actor 替换。

## IPC 与保存

- `pdf_read_ink({vaultPath,paperFolder,page})` 返回 `InkPageSnapshot`：已验证的 `document`、精确原字节 SHA-256 `revision`（仅文件确实缺失为 `null`），以及创建/更新时间的十进制字符串映射。
- `pdf_apply_ink({...expectedRevision,operations})` 接收一组 put/delete 草稿。put 不接收 revision；创建和更新时间仅通过不透明字符串保留历史值，前端不递增或往返转换 `u64`。编辑点时间限 `u32`，不能精确表示的旧点时间拒绝写回。
- 两个命令均为异步 `Result<ApiResult<T>,String>`，并登记在桌面、移动白名单和生成绑定中。移动端只接受应用私有 Vault，paperFolder 必须为规范的 `papers/<单目录>`，页码为正数。

读取在共享文件变更锁内取得同一份原字节与模型。写入在同一锁内重读、比较精确 revision、生成新逻辑版本并发布；与同步/编辑器的协作写入发生竞争时返回 `ink_conflict`，保留当前文件和用户草稿。未知版本、损坏文件、路径逃逸、父级/叶子链接和超限数据均报错，不能退为空页。边界为 16 MiB/页、100,000 点/页、20,000 点/笔、10,000 条记录/页及 256 个操作/批。

唯一同目录临时文件先 flush，再原子替换，Unix 随后 flush ink、marks 与论文目录。同步保存提示只在成功后、共享锁外发送。写入前取消可以阻止发布；进入同步发布段后完成原子操作，不能声称撤回已进入的保存。重命名后的目录 flush 失败返回 `ink_unavailable`，新内容可能已可见；调用方保留同一 UUID 的草稿并重读核对，不能按错误假定旧字节仍在。

该锁保护进程内协作写入，并非针对任意外部进程的操作系统 CAS。同步仍须使用相同的原字节检查与持久发布保证，不能把已确认的本地笔迹替换成未 flush 的合并文件。

## 前端与导出

[ink-api.ts](../../../src/lib/pdf/ink-api.ts) 只调用生成绑定并保留错误码。文档控制器按页排队 CAS 草稿；保存失败保留原 UUID、预览和 dirty 状态，重试需显式触发。撤销、重做、按笔擦除、套索移动/复制/删除都产生原生新 revision；删除写墓碑。未知保存重新读到同一目标内容时保留原撤销意图；同 ID 的外部修改与目标不同，则重试以用户已看到的当前内容重建逆操作。新笔的原生创建/更新时间也保存到历史，重做不生成新的创建时间。

渲染使用固定的 [perfect-freehand 1.2.3（MIT）](https://github.com/steveruizok/perfect-freehand) 生成压力轮廓，保留原始点而非生成的屏幕多边形。输入使用合并 pointer samples；手指平移复用 EmbedPDF 的公开 viewport scrolling，双指缩放交给已有 ZoomGestureWrapper。笔输入期间隔离掌触，cancel、blur 和变换变化取消当前手势。共享数据控制器与每视图的工具状态分开，笔记里的只读嵌入不会关闭主阅读器的书写工具。

一笔成形（hold-to-shape）只对笔笔迹生效：笔未抬起且停止移动 500 ms（`INK_SHAPE_HOLD_MS`，6 CSS px 抖动容差）时，[shapes.ts](../../../src/lib/pdf/ink/shapes.ts) 对已积累采样做识别，把草稿预览替换为规范图形；抬笔提交该图形，继续绘制则恢复原始手写，抬笔点远离停留点也按手写提交。支持直线、箭头、矩形、三角形、圆/椭圆：先用 [simplify-js 1.2.4（MIT）](https://github.com/mourner/simplify-js) 做 Ramer–Douglas–Peucker 简化，按角点数量分类，再对原始采样验证拟合置信度；手写、弧线、对勾、菱形等置信度不足时保留墨迹。规范图形仍存为普通点序列：压力取原始采样均值，点时间按弧长在绘制时长内分配，倾角为空，因此撤销/重做、同步与副本导出不变；吸附预览按完成笔迹渲染。橡皮与套索不触发，cancel、blur、缩放仍丢弃草稿。ML 识别方案（如 desert-ant-labs shapes）因 WASM 体积与限制性 source-available 许可未采用。

副本导出先用公开 `engine.saveAsCopy` 取得当前 PDF，再通过已有 `pdf-lib` 添加标准 Ink、原始 InkList 及带压力轮廓的 `/AP` Form。保留 CropBox、旋转、原有标注和源字节；不把导出时的临时标注写入开放文档或 sidecar。移动端复用已安装的官方 dialog/fs 插件：Android SAF content URI 和 iOS security-scoped file URL 原样交给文件插件。默认文件名是受限且唯一的 basename，选择原论文目录内的目标会被拒绝。文件分享面板属于后续平台入口；原生保存仍需真机验证。

回归覆盖并发 CAS、原字节保留、未知/超限文件、路径和 actor 隔离、删除/撤销/重做及超出 JavaScript 安全范围的原生元数据。前端还覆盖指针取消、手指导航、模式切换、未知保存、冲突后撤销，以及由独立 PDFium 解析并栅格化四种裁剪/旋转副本。模拟 pointer 事件和桌面原生预览不能替代 Apple Pencil、Android 手写取消、延迟与设备内存验收。
