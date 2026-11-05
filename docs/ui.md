# 应用壳与界面

`src/app.rs` 负责把「面板上的选择」变成一次任务、再把结果放回面板。`src/ui/` 只负责画，
不碰 PDF 与图片类型（见 [architecture.md](./architecture.md) 的依赖方向）。

## 1. 布局契约

面板创建顺序就是布局契约：先 `Panel::left`（侧边栏），再 `Panel::bottom`（结果卡 + 操作条），
最后 `CentralPanel`。

顺序反了会出真问题：先建底部面板时，结果卡横跨整个窗口并吃掉侧边栏高度，跑完一次任务就
把「加密与解密 / PDF 转图片」挤出可视区——看起来像少了两个工具。

卡片用 `theme::card` / `tinted_card`，内部 `set_min_width(ui.available_width())` 撑满内容列；
按内容自适应会得到参差不齐的卡片堆。

## 2. 任务状态机

```
Idle → Ready → Running{fraction, message} → Done{summary, outputs, note} | Failed | Cancelled
```

每个工具各存一份状态，切走再切回时内容仍在。

- **同一时刻只允许一个任务**：`start()` 在 `self.worker.is_some()` 时直接返回。切换工具时仍在
  跑的任务由底部条显示「X 处理中」并提供取消；早期实现会覆盖 `self.worker`，前一个任务句柄
  丢失、状态永远停在「处理中」。
- **进度**：worker 侧 `Progress::report(fraction, msg)`，UI 每帧读一次，不做跨线程锁。
- **取消**：`CancelFlag` 置位后工具在下一个检查点返回 `Cancelled`，已写出的临时文件随之丢弃。

## 3. 同名文件对话框

工具返回 `ToolError::OutputExists { paths }` 后，`poll_worker` 把它转成模态提问（而不是失败卡）：

- 选择「覆盖」→ 以 `Overwrite` 重跑；
- 选择「另存为新文件」→ 以 `Unique` 重跑，序号自动加 `-2`、`-3`；
- 取消 → 什么都不做。此时工具尚未写盘，无需清理半成品。

勾选「本次后续均执行此操作」写入会话策略，底部操作条显示「同名文件：覆盖 / 另存为新文件」
并提供「重置」。策略只存在于内存，重启即清空——写进偏好文件会让用户在某天发现文件被静默
覆盖却找不到开关。

## 4. 输出路径解析

用户指定优先；否则与源文件同目录；目录不可写时按下列顺序回退，并在结果区标注「已改存到…」：

```
文档\SparrowPDF → 文档 → OneDrive\文档\SparrowPDF → OneDrive\文档
→ 桌面 → 下载 → 用户目录 → TEMP
```

结果缓存，避免每帧探测。执行前 `preflight_output` 向上寻找最近的已存在祖先判断可写性，
权限不足立即给出中文提示，而不是让工具跑到一半才失败。

## 5. 主题与外观

所有颜色都从 `theme.rs` 的语义函数取（`accent` / `success` / `warning` / `danger` /
`card_fill` 等），按 `ui.visuals().dark_mode` 分支，切换外观即时生效、无需重启。

- 首选跟系统主题；用户手动选择后写 `%LOCALAPPDATA%\sparrowpdf\ui.pref`，下次启动沿用。
- 输入框统一用 `theme::text_input` / `password_input` / `number_input`，宽度按剩余行宽
  clamp，长文案走 tooltip 而不是硬塞占位符。
- 界面文案不使用 emoji 与生僻符号（系统 CJK 字体缺字形会显示成方框）。

## 6. 预览与拖放

- 拖放按当前工具接受的文件类型过滤；拖入顺序会被整理（PDF 自动排到第一槽）。
- 缩略图逐帧限流渲染，密码变更时清缓存重开重渲染（详见 [rendering.md](./rendering.md) 第 7 节）。
- 空输入不渲染空卡片；页码标签与悬停提示区分「图片 N」与「页 P」（两图一页时二者不同）。

## 7. 已知缺口

- 会话级「本次后续」策略无法在界面内跨任务重置，只能重启应用或点底部条的「重置」。
- 结果卡只保留最近一次任务，没有历史列表。
- 窗口最小 760×520，更窄时侧边栏与面板会压缩而不是滚动。
