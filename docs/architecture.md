# 分层与模块职责

## 1. 分层结构

```
main.rs ── 启动窗口、内嵌图标、运行 eframe
   │
app.rs ─── 应用壳：工具切换、表单状态、输出路径解析、worker 轮询、结果渲染
   │
   ├── ui/        纯表现层（不得出现 PDF / 图片库类型）
   │    ├── theme.rs     设计令牌 + 组件（卡片、按钮、输入框、状态色、结果区、外链打开）
   │    ├── sidebar.rs   工具导航 + 外观切换 + 关于入口
   │    ├── status.rs    底部操作条（主按钮、进度、结果区）
   │    ├── about.rs     关于弹窗（版本、作者、项目地址）
   │    ├── conflict.rs  同名文件模态提问
   │    ├── preview.rs   缩略图预览（图片与 PDF 页面，逐帧限流渲染）
   │    ├── prefs.rs     外观偏好持久化（%LOCALAPPDATA%\sparrowpdf\ui.pref）
   │    └── panels/      每个工具一个面板文件，与 tools/ 一一对应
   │
   ├── tools/     业务层：把一次请求变成产物
   │    ├── mod.rs        Tool trait、ToolOptions、Progress、CancelFlag、tool_for
   │    ├── image_embed.rs 图片页嵌入与排版（被图片类工具共享）
   │    └── <tool>.rs     12 个工具实现
   │
   ├── engine/    基础设施层：不含业务语义
   │    ├── pdf_io.rs   打开/保存/原子写/可写性探测/错误文案
   │    ├── worker.rs   后台线程 + catch_unwind + 取消
   │    ├── validate.rs 产物断言（页数、方向、非空文本等）
   │    ├── render.rs   PDF 页面软件光栅化（缩略图与 PDF 转图片）
   │    ├── raster.rs   像素缓冲与绘制原语
   │    ├── ttf.rs      TrueType/OpenType 解析（支持 .ttc 集合）
   │    └── logging.rs  追加式文件日志
   │
   ├── model.rs   领域模型（无 IO、无第三方 PDF 类型）：ToolId / ToolGroup / ToolError / TaskState
   └── version.rs 版本号唯一来源：Cargo 三段 + 日期位
```

依赖方向是硬约束，反向引用会让分层失效（`ui/` 一旦拿到 `lopdf::Document`，
预览与业务就会各自演化出两套页面语义）：

- `ui/` → `model`、`tools`（选项类型、trait 签名，以及预览所需的纯函数：
  `collect_images`、`preview_page_numbers`、`ordered_pages`）、`engine::logging`
- `tools/` → `engine`、`model`、`lopdf` / `medpdf` / `image`
- `engine/` → `model`（错误类型）与各自领域库
- `model.rs` → 仅标准库
- `ui/` 中禁止出现 `lopdf`、`medpdf`、`medpdf_image`、`image`、`kamadak_exif` 的类型
  （`src/ui/mod.rs` 顶部有明文约束）

## 2. 一次任务的调用链

```
面板收集 ToolRequest
  → app::start() 起 worker 线程（同一时刻只允许一个任务）
    → Tool::validate() 只读预检，尽早失败
    → Tool::execute(progress, cancel)
      → 逐项处理：check_cancel + progress.report
      → pdf_io::resolve_outputs() 解析同名冲突
      → pdf_io::save_document() 临时文件 + rename 落盘
      → validate::assert_* 重新打开产物断言
  → poll_worker() 把 ToolOutcome / ToolError 映射成结果卡
```

跨线程只有三样东西：`Progress`（进度回传，内部限幅 0~1）、`CancelFlag`
（`Arc<AtomicBool>`）、`ToolOutcome`。UI 线程不做解码与光栅化，大文档一律走 worker，
预览按帧预算渲染。

## 3. 平台集成

**日志**：`engine::logging::log("INFO"|"WARN"|"ERROR", msg)`，追加写入
`%LOCALAPPDATA%\sparrowpdf\sparrowpdf.log`；`SPARROW_LOG_DIR` 可覆盖（测试与便携使用）。
发布版无控制台，所以不要用 `println!`。

**Windows 清单**（`build.rs`）：`requireAdministrator` + `longPathAware` + `dpiAware`。
清单只注入主程序（`winresource::compile_for("sparrowpdf")`），否则测试可执行文件也会被
要求提权，`cargo test` 直接跑不起来。

**免提权变体**：`SPARROW_AS_INVOKER=1 cargo build --release`，清单改为 `asInvoker`。
适合按用户安装、需要资源管理器拖放正常的场景；写受保护目录会失败，此时程序按
`ui.md` 的回退链改存到 `文档\SparrowPDF`。

**macOS**：`app.icns` + `CFBundleIconFile`，`CFBundleVersion` 用日期位当 build 号，
`CFBundleShortVersionString` 用三段核心版本。打包为通用二进制（arm64 + x86_64 经
`lipo` 合并）并做 ad-hoc 签名；正式分发仍需 Developer ID 与公证，否则 Gatekeeper
会拦下首次打开。

## 4. 已知缺口

- 日志不滚动、不清理：长期使用会持续增长，排查完可手动删除。
- 偏好文件只有外观一项，损坏时静默回落到跟随系统，不报错。
- macOS 未做公证，用户需自行解除隔离（README 有一行命令）；这是发布策略，不是代码缺陷。
