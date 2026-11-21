# 轻雀 PDF（SparrowPDF）协作与交付规范

> 本文件是仓库内**唯一**的工程协作契约，面向人类开发者与 AI 编码代理（Claude Code、Codex、Cursor、Copilot Workspace 等）。  
> 与 `README.md` 冲突时，以本文件为准；与代码现状冲突时，先修文档或代码，不允许两套标准并行。  
> 维护责任：发布负责人。每次流程变更必须同步修改本文件，并随代码一起提交。

---

## 0. 使用方式（AI 代理执行约定）

开始任何任务前，按以下顺序建立上下文，完成后再动手：

1. 读 `Cargo.toml`（版本号、依赖、release profile）
2. 读 `src/model.rs`（`ToolId`、`ToolGroup`、`ToolError`、`TaskState`）
3. 读 `src/tools/mod.rs`（`Tool` trait、`ToolOptions`、`Progress`、`CancelFlag`）
4. 读 `docs/` 里对应主题的实现说明（工具 / 渲染 / 界面 / 分层），再读将要修改的模块
5. 若涉及界面，再读 `src/ui/theme.rs`（设计令牌与组件）

本文件只写**规范**（约束、流程、质量门）；**怎么实现、为什么这么做、哪里会出问题**
写在 `docs/`，按主题查阅即可。

执行原则：

- **先读后写**：不得在未读源码的情况下凭猜测改接口。
- **分支默认 `dev`**：所有改动提交到 `dev`；`main` 只由发版合并推进（见 6.6），未经用户明确要求不得改动 `main`。
- **最小改动**：一次任务只解决一个议题，不夹带格式重排、无关重命名。
- **验证前置**：改完必须跑第 4.4 节的质量门，输出真实结果，不臆测「应该能过」。
- **不做外部动作**：`git push`、`git tag` 推送、创建 Release、修改仓库设置等一律需人工明确授权。
- **不确定就问**：涉及依赖增删、公开接口破坏、License 变更时停下确认。

---

## 1. 项目概览

### 1.1 定位与硬性约束

| 项目 | 说明 |
| --- | --- |
| 产品名 | 轻雀 PDF / SparrowPDF |
| 形态 | 单机桌面应用（Windows / macOS），单文件可执行 |
| 语言 | Rust，edition 2021，MSRV 1.95（CI 固定 1.97.1） |
| 界面 | egui / eframe 0.37，Glow 后端，无 WebView |
| 数据流 | **纯本地**：不联网、不上传、无遥测、无账号体系 |
| 工具数 | 12 项，分组：转换 3 / 结构操作 6 / 页面编辑 2 / 安全 1 |
| 安装形态 | 绿色 exe（Windows x64 / arm64）、`.app` 打包为 DMG（macOS 通用版） |

破坏性约束（任何改动都不得违反）：

1. 不引入网络请求、埋点、崩溃上报、自动更新检查。
2. 不改动 `LICENSE`（MIT，版权人 霖海）。
3. 不提交二进制产物、构建缓存、本地调试素材。
4. 新增依赖仍须过第 5.8 节的论证（体积、许可证、维护状态），并在 1.2 节登记。

### 1.2 技术栈

| 依赖 | 用途 |
| --- | --- |
| `eframe` / `egui` 0.37 | 窗口与 immediate-mode UI |
| `egui_dnd` 0.18 | 页面/文件拖拽排序 |
| `rfd` 0.17 | 系统文件/文件夹选择对话框 |
| `lopdf` 0.39 | PDF 对象模型、读写、加密。**已知风险**：0.39 解析嵌套数组/字典没有递归上限，构造的 PDF 可让进程栈溢出退出（GHSA-6fj4-rcr3-r7rr，0.42.0 起修复）。上限由 `medpdf` 锁死（`lopdf ^0.39`），等上游放宽后一起升 |
| `medpdf` 0.9.2 / `medpdf-image` 0.2.2 | 文本度量、图片编解码与嵌入。两者都声明 `lopdf ^0.39`，因此本项目的 `lopdf` 只能停在 0.39；**不要**单独升 `lopdf`（会出现两个版本并存、与 medpdf 类型不匹配而编译失败） |
| `kamadak-exif` 0.6 | JPEG 的 EXIF 方向读取 |
| `image` 0.25（jpeg, png） | 图片解码、PNG 编码 |
| `fast_image_resize` 6.1（std） | 图片重采样（AVX2 / NEON 运行时派发，Lanczos-3） |
| `jpeg-encoder` 0.7（simd） | JPEG 编码（4:4:4、优化霍夫曼表；熵编码源自 IJG） |
| `allsorts` 0.16（flate2_rust） | 系统 TrueType 字体解析（用于内置光栅器与中文水印） |
| `weezl` 0.2 | LZW 解码（图像 `/LZWDecode`） |
| `hayro-ccitt` 0.5 | CCITT G3/G4 传真解码（`/CCITTFaxDecode`）。**与 `hayro-jbig2` 同批升级**：jbig2 0.4 自身依赖 ccitt 0.5，两者错开会在锁文件里留下两份传真解码器（CI 的「直接依赖版本唯一性」会拦下）；0.5 的解码回调整行交给 `push_row`，不再逐像素 |
| `hayro-jbig2` 0.4（std） | JBIG2 解码（`/JBIG2Decode`），含共享全局段。0.4 起入口是 `Page` + `Globals`（原先的 `Image` 已移除）。**已知风险**：`Globals::new` 会立即展开全部图案字典，展开量与页面尺寸无关，构造的字典可让小输入吃满内存（CWE-400）。上游没有累计展开上限，本项目在 `render.rs` 用 `MAX_GLOBALS`（4 MiB）拒绝超大的共享字典，代价是该图被跳过；等上游提供计量接口再换成真正的限额 |
| `flate2` 1（rust_backend） | zlib 解压（图像 `/FlateDecode` 与预测器流水线） |
| `windows-sys` 0.61（registry / system-information 特性，仅 Windows） | 「关于」读取系统版本与 CPU 型号，线程池读取物理内存；同版本已在依赖树中（`winresource` 带入），只开这两组 API |
| `winresource` 0.1（build-dep） | 向 Windows exe 注入图标与清单 |

---

## 2. 目录结构与职责

### 2.1 顶层结构

```
SparrowPDF/
├── Cargo.toml            # 包元数据、依赖、release profile（体积/性能开关）
├── Cargo.lock            # 锁定依赖，发布构建使用 --locked
├── .cargo/audit.toml     # cargo audit 的例外登记（目前只有 lopdf 一条，见 1.2）
├── build.rs              # Windows 资源：app.ico + 应用清单（提权/长路径/DPI）
├── assets/               # 图标素材（icns / ico / png），不参与运行时解码逻辑
├── src/                  # 全部 Rust 源码（见 2.2）
├── tests/integration.rs  # 端到端集成测试（不启动 GUI，直接调 Tool）
├── README.md             # 面向用户的功能、下载与开发/贡献说明
├── AGENTS.md             # 本文件：面向协作者的工程规范
├── docs/                 # 技术实现文档（分层、工具、渲染、界面），改代码前读对应主题
└── .github/
     ├── workflows/         # release.yml：tag 触发，产出双平台产物；ci.yml：PR 与 dev 检查（4.6）；
     │                      # deps-merge.yml：依赖 PR 分流与自动合并（4.7）
     ├── dependabot.yml     # 依赖更新：打到 dev、hayro 家族并组、lopdf 停在 0.39.x（4.7）
     └── pull_request_template.md  # PR 模板，开 PR 时自动填充
```

本地开发期目录（**不入库**，已在 `.gitignore` 中）：`target/`、`.probe/`（截图与调试素材）、`.workbuddy/`（本机代理工作数据）。

### 2.2 `src/` 分层与依赖方向

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
   │    ├── preview.rs   缩略图预览（图片与 PDF 页面，逐帧限流渲染）
   │    ├── prefs.rs     外观偏好持久化（%LOCALAPPDATA%\sparrowpdf\ui.pref）
   │    └── panels/      每个工具一个面板文件，与 tools/ 一一对应
   │
   ├── tools/     业务层：把一次请求变成产物
   │    ├── mod.rs        Tool trait、ToolOptions、Progress、CancelFlag、tool_for
   │    ├── image_embed.rs 图片页嵌入与排版（被图片类工具共享）
   │    └── <tool>.rs     12 个工具实现（image_to_pdf、folder_to_pdf、merge、…）
   │
   ├── engine/    基础设施层：不含业务语义
   │    ├── pdf_io.rs   打开/保存/原子写/可写性探测/错误文案
   │    ├── pool.rs     有界线程池：任务队列、取消传播、panic 隔离
   │    ├── worker.rs   后台线程 + catch_unwind + 取消
   │    ├── validate.rs 产物断言（页数、非空文本等）
   │    ├── render.rs   PDF 页面软件光栅化（缩略图与 PDF 转图片）
   │    ├── raster.rs   像素缓冲与绘制原语
   │    ├── ttf.rs      TrueType/OpenType 解析（支持 .ttc 集合）
   │    ├── sysinfo.rs  本机信息（系统版本、CPU 型号），只读且按会话缓存
   │    └── logging.rs  追加式文件日志（%LOCALAPPDATA%\sparrowpdf\sparrowpdf.log）
   │
   ├── model.rs   领域模型（无 IO、无第三方 PDF 类型）：ToolId / ToolGroup / ToolError / TaskState
   └── version.rs 版本号唯一来源：Cargo 三段 + 日期位，拼出 V主.副.修订.YYYYMMDD
```

**依赖方向（硬约束）**

- `ui/` → `model`、`tools`（选项类型、trait 签名，以及预览所需的纯函数：`collect_images`、`preview_page_numbers`、`ordered_pages`）、`engine::logging`
- `tools/` → `engine`、`model`、`lopdf` / `medpdf` / `image`
- `engine/` → `model`（错误类型）与各自领域库
- `model.rs` → 仅标准库
- `ui/` 中**禁止**出现 `lopdf`、`medpdf`、`medpdf_image`、`image`、`kamadak_exif` 的类型（`src/ui/mod.rs` 顶部有明文约束）

### 2.3 关键文件职责

| 文件 | 职责 | 修改风险 |
| --- | --- | --- |
| `src/model.rs` | 工具注册表与错误模型；新增工具必须在此登记 | 高：影响全部面板与测试 |
| `src/tools/mod.rs` | `Tool` 契约、选项枚举、进度/取消通道、工厂函数 | 高 |
| `src/engine/pdf_io.rs` | 唯一的落盘入口（`save_document` / `write_file_atomic` 原子写，`write_file` 直写）、密码打开、可写性探测 | 高：写坏即数据丢失 |
| `src/engine/worker.rs` | 线程生命周期、panic 捕获、取消传播 | 中 |
| `src/engine/pool.rs` | 逐项并发调度（任务队列、失败策略、panic 隔离、结果回传） | 中高：并发缺陷难复现 |
| `src/app.rs` | 输出路径解析与回退、任务编排、结果状态机 | 中高 |
| `src/ui/theme.rs` | 全部视觉令牌与通用控件 | 中：改动影响所有面板 |
| `build.rs` | Windows 清单与图标；提权策略 | 中：可令程序无法启动 |

---

## 3. 核心功能与契约

### 3.1 工具清单

| 分组 | 工具 | 输入 | 产物 |
| --- | --- | --- | --- |
| 转换 | 图片合并为 PDF | 多张 JPG/PNG | 单个 PDF |
| 转换 | 批量转换 | 多个文件夹 | 每个文件夹一个 PDF |
| 结构操作 | PDF 合并 | ≥2 个 PDF | 单个 PDF |
| 结构操作 | PDF 拆分 | 1 个 PDF | 按页码范围 / 每 N 页 / 逐页输出 |
| 结构操作 | 页面旋转 | 1 个 PDF | 旋转后的 PDF |
| 结构操作 | 删除页面 | 1 个 PDF | 删页后的 PDF |
| 结构操作 | 页面重排 | 1 个 PDF | 重排后的 PDF |
| 结构操作 | 提取页面 | 1 个 PDF | 抽取页组成的新 PDF |
| 页面编辑 | 文本水印 | 1 个 PDF | 叠加文字水印的 PDF |
| 页面编辑 | 图片水印 | 1 个 PDF + 1 图 | 叠加图片水印的 PDF |
| 安全 | 加密 / 解密 | 1 个 PDF | AES-256 加密或移除密码 |
| 转换 | PDF 转图片 | 1 个或多个 PDF | 每页 PNG/JPEG |

### 3.2 统一契约

所有工具实现 `Tool`（`src/tools/mod.rs`）：

```rust
pub trait Tool: Send + Sync {
    fn id(&self) -> ToolId;
    fn validate(&self, req: &ToolRequest) -> Result<(), ToolError>;   // 只读预检
    fn execute(&self, req: &ToolRequest, progress: &Progress,
               cancel: &CancelFlag) -> Result<ToolOutcome, ToolError>;
}
```

- `ToolRequest`：`inputs: Vec<InputFile>`、`output: PathBuf`、`options: ToolOptions`、`password: Option<String>`、`conflict: ConflictPolicy`。
- `ToolOptions`：每工具一个变体，保证 `execute` 签名统一且类型安全。
- `Progress::report(fraction, msg)`：把进度送回 UI（内部限幅 0~1）。
- `CancelFlag = Arc<AtomicBool>`：**循环体内**必须调用 `check_cancel(cancel)?`。
- 成功返回 `ToolOutcome { summary, outputs }`。
- 部分失败返回 `ToolError::PartialSuccess { succeeded, failed, detail, outputs }`，**必须**携带已产出的文件：应用壳把它映射成「已完成 + 注意」的结果卡，用户仍能打开产物；缺 `outputs` 会把可用的结果显示成一次彻底失败。
- **同名文件**：工具必须先生成本次要写的全部路径，再经 `pdf_io::resolve_outputs(&paths, req.conflict)` 换取最终路径，之后才可落盘。`ConflictPolicy::Report`（默认）返回 `ToolError::OutputExists { paths }` 且不写任何文件；`Overwrite` 替换；`Unique` 追加 `-2`、`-3`。禁止绕过这套解析直接写 `req.output`，也不得在工具内部弹窗或自行询问。

### 3.3 落盘与自校验流水线（所有工具必须遵守）

1. 解析参数 → 尽早失败（`validate`）。
2. 逐项处理，每步 `check_cancel` + `progress.report`。
3. **统一经 `pdf_io` 的落盘入口写入，并把 `req.conflict` 一并传入**：`save_document`（文档）与 `write_file_atomic`（已序列化字节）先写同目录临时文件再 `rename`，`write_file` 直接写——常规产出优先用前两个，只有调用方明确接受非原子写时才用 `write_file`（如逐页导出的小文件）。三者都会在写之前再复核一次目标是否存在（`ConflictPolicy::Report` 时直接返回 `OutputExists`）；权限错误由 `write_error_detail` 映射为中文提示。规划与落盘之间可能隔几分钟的渲染，这次复核是防静默覆盖的第二道闸。
4. 落盘后**重新打开产物断言**（`validate::assert_page_count` 等）；断言失败要删除产物并返回 `ValidationFailed`。
5. 不得在工具内部直接 `fs::write` 或 `File::create` 绕过第 3 步。

### 3.4 实现细节

具体实现按主题写在 `docs/`，这里不再展开：

| 要改的东西 | 先读 |
| --- | --- |
| 某个工具的处理逻辑 | `docs/tools.md` |
| 渲染、导出质量 | `docs/rendering.md` |
| 界面、布局、任务状态 | `docs/ui.md` |
| 分层、新增模块、平台 | `docs/architecture.md` |

读法：先读本节 3.2 / 3.3 的契约，再读对应文档；各篇末尾的「已知缺口」为既有边界，
不应作为回归缺陷处理。

### 3.5 图片与扫描件的分辨率与体积契约

图片类工具（图片合并为 PDF、批量转换）共用 `tools/image_embed.rs` 的一条流水线。
以下六条是改这条流水线时必须保持的行为：

1. **按绘制尺寸算分辨率**：有效 DPI = 源像素 ÷ **绘制后**的页面尺寸，不是纸张尺寸
   （等比缩放留白时用纸张会高估所需像素）。超出预算才重采样（`fast_image_resize` 的
   Lanczos-3）并重编码；不超预算的 JPEG 保持原字节直通，不做二次编码。
2. **档位与默认值**：批量转换的「体积优化」为 `ScanQuality`——`标准 = 300 DPI / 质量 85`
   （默认）、`更小体积 = 150 DPI / 质量 78`、`不压缩 = 保留源像素`。该选项是用户偏好，
   不随模板切换重置（`apply_batch_template` 不得改动它）。
3. **不透明 PNG 存 JPEG**：alpha 全 ≥ 250 视为不透明并丢弃 `/SMask`；有真实透明时仍走
   `/FlateDecode` + `/SMask`。任何位图流都必须显式 `Stream::compress()` 后再落盘——
   lopdf 的保存路径不会自动压缩，漏掉这一步一页 600 DPI 扫描就是约 100 MB 裸像素。
   JPEG 一律 4:4:4 + 优化霍夫曼表（`jpeg-encoder`）：在扫描件上比 4:2:0 默认档更小也更
   保真。**不要**把 `set_optimized_huffman_tables` 与 4:2:0 组合——该 crate 的非交织
   路径会写出损坏的码流（实测 PSNR 2.4 dB），需要次采样时先确认版本已修复。
4. **20 MB 上限**：批量转换单份 PDF 超过 `SCAN_SIZE_LIMIT`（20 MiB）时，按
   `ScanBudget::tightened`（DPI × 0.82、质量 −6，下限 96 DPI / 45）收紧后整份重渲染；
   文档先在内存里序列化量出精确体积，**只落盘一次**。到下限仍超出必须在结果摘要里
   如实写明「压缩后仍超过 20 MB」。
5. **方向只进矩阵**：EXIF 方向与槽位自动旋转都只体现在绘制矩阵上，像素不做物理旋转；
   目标像素数按显示方向计算，再按「EXIF 与旋转异或」映射回源方向——漏掉映射会把合并页
   （旋转 90° 进半页槽位）的图片按错误长宽比重采样，出现横向拉变形。
6. **准备与组装分离**：`prepare_image`（解码 → 重采样 → 编码）不得接触 `Document`，
   `add_page` 只写 PDF 对象。批量转换据此并行准备图片、串行组装；新增图片处理逻辑必须
   留在这条边界内，否则并行、进度与失败隔离都会失效（并发规则见 3.6）。

### 3.6 并发执行的任务边界

CPU 密集的逐项工作统一走 `engine::pool`，两个工具共用一套调度与失败规则：

1. **进池子的只有纯计算**：批量转换的图片准备（`prepare_image`）、PDF 转图片的页面
   栅格化。二者都不修改 `Document`、不写盘，因此可以乱序完成。
2. **留在调用线程的一律串行**：文档对象增删、`resolve_outputs` 冲突解析、体积守卫的
   序列化与落盘、结果汇总，以及逐件产物的**写盘与 `validate::assert_*` 校验**（池任务只
   返回字节与尺寸）。它们要么依赖页序，要么共享可变状态或触碰文件系统，并发化只会换来
   竞态。
3. **线程数按机器算**（`pool::worker_count`）：取「核数 − 1」「物理内存 / 3 ÷ 单任务
   峰值」「`MAX_WORKERS` = 8」三者的最小值；内存读不到时取上限而不是猜低值。单任务峰值
   由调用方声明（图片准备 400 MB，页面栅格化 192 MB），不要凭感觉改。
4. **失败策略显式声明**：`OnError::Stop`（一张扫描件坏了，整份 PDF 没有意义，整夹跳过）
   与 `OnError::Continue`（某一页渲染失败，其余照常导出）。默认应选 `Stop`。
5. **异常兜底在池内**：每个任务包在 `catch_unwind` 里，panic 变成该任务的
   `ToolError::Internal` 并带上是第几项；线程 `join` 的失败也要转成错误，不允许静默少项。
6. **资源上限固定**：结果通过容量为 `workers` 的 `sync_channel` 交回，完成项不会随页数
   堆积；每个任务的结果被消费后立即释放（解码缓冲随作用域结束回收）。
7. **共享只读数据必须 `Sync`**：页面栅格化把 `&Document` 与 `&FontSet` 直接分给线程，
   字体字形缓存因此是 `Mutex` 而不是 `RefCell`——不要为了省锁把它改回去，那会让
   整个 `FontSet` 无法跨线程共享，退化成每线程解析一套字体。

体积与时序基线（16 核 / 16 GB，13 页 600 DPI 扫描件）：批量转换 300 DPI 约 6 MB / 2 秒，
对照的第三方排版产物为 5.2 MB（其分辨率 1876–3296 px/页，本工具统一 2479 px/页，每像素
字节数持平）；PDF 转图片 300 DPI 12 页约 2.5 秒；同一批在不压缩档约 16 MB。出现数量级
偏离时先查 3.5 的第 3 条与第 5 条。

---

## 4. 构建与验证

### 4.1 常用命令

```bash
cargo clippy --release --all-targets -- -D warnings   # 质量门：类型检查 + 警告清零
cargo test --release --lib --test integration         # 单元 + 集成测试
cargo audit                                           # 依赖漏洞（动过依赖时跑）
cargo build --release --locked                        # 发布构建（CI 同款）
```

### 4.2 Windows 构建环境要点

- 需要 MSVC 与 Windows SDK；若 `cl.exe` 报 `C1083 无法打开 stddef.h`，说明当前 shell 缺 `INCLUDE`/`LIB`，改用 VS 开发者命令提示符。
- debug 构建可能触发 `libz-sys` 的 `D8050`（与 `-Z7` 相关）——验证功能优先用 `--release`。
- `panic = "unwind"` 是**功能依赖**：worker 依赖 `catch_unwind` 把 panic 转成可见错误，不得改回 `abort`。
- `strip = true` + `lto = true` + `opt-level = "z"` 共同压体积；CI 有 20 MB（UPX 前）硬门禁与 12 MB 软目标。
- 免提权变体（`SPARROW_AS_INVOKER=1`）写受保护目录会失败，程序回退到 `文档\SparrowPDF`；需要直写时对目标目录一次性授权：`icacls "目标目录" /grant "%USERNAME%":(OI)(CI)M`。

### 4.3 测试策略

| 层级 | 位置 | 覆盖内容 |
| --- | --- | --- |
| 集成测试 | `tests/integration.rs`（用例数随代码增长，不在此处记数字） | 工具端到端：页码解析、合并、旋转、删页、加密往返、拆分、中文水印位图断言、日志追加、图片自动旋转、批量排序/两图一页/非法序号/合并页槽位顺序；同名文件三种策略（Report/Overwrite/Unique）与落盘前复核、重命名失败保留旧产物、LZW / CCITT / JBIG2（含有效码流）/ PNG 预测器 / 原始采样图像渲染、CID 文本经 `/ToUnicode` 取字（含 `bfchar` 计数与 `bfrange` 数组）、`BT` 跨块保留字体、`TJ` 连续排版与 `Tz` 缩放、低位深按行解包、图像行序与旋转变换、内嵌 `/FontFile2` 无 ToUnicode 出字、扫描件存 DCTDecode 的像素尺寸与 SMask 取舍、体积守卫收紧阶梯、坏图只跳过所在文件夹、导出页写盘失败按页报告 |
| 单元测试 | 模块内 `#[cfg(test)]` | 纯函数（码表、偏好文件读写等） |

规则：

- 新增或修改工具行为 → **必须**补一条集成测试，断言产物结构（页数、MediaBox、矩阵、XObject 等），不只断言「执行成功」。
- 测试的临时目录走 `CARGO_TARGET_TMPDIR`（工作区内），不写系统 temp，避免受限环境失败。注意它只存在于**编译期**环境，必须用 `option_env!` 读取，运行时 `std::env::var` 永远取不到（会静默退回系统 temp）。
- 测试夹具由测试自行生成（`make_pdf` / `make_image`），不依赖仓库内二进制。

### 4.4 提交前质量门（必须全绿）

1. `cargo clippy --release --all-targets -- -D warnings` 零警告。clippy 同时做类型检查，
   所以它替代了原来的 `cargo check`；只想快速看能不能编译时仍可单跑 `cargo check`。
2. `cargo test --release --lib --test integration` 全通过。
3. 依赖漏洞：`cargo audit` 无新增告警（CI 每次必跑；本地在动过 `Cargo.toml` / `Cargo.lock`
   时跑一次，新增告警按 5.8 处理或写清理由）。已知例外只有 `.cargo/audit.toml` 里登记的
   lopdf 那一条，原因见 1.2；登记新的例外必须在同一处写明为什么无法升级。
   动过依赖时再看一眼 `cargo tree -d`：**直接依赖**不得出现同一 crate 的两个版本
   （CI 同款门禁，判据与例外登记见 4.7）。
4. 若改动涉及 UI：至少一次实际启动（`cargo build --release` 后运行）确认无渲染异常、无布局裁切。
5. 若改动版本号：`Cargo.toml` 与 `Cargo.lock` 同步，且与将要打的 tag 一致。
6. 若改动发布流水线：说明验证方式（无 actionlint 时至少核对结构与关键参数）。
7. PR 上「质量门（Windows）」状态检查必须通过才可合并（4.6）；本地绿而 CI 红时按 CI 日志修，不得绕过检查合并。

### 4.5 发布流水线（`.github/workflows/release.yml`）

**触发与版本来源**

- 推 tag（`V主.副.修订.YYYYMMDD`）自动运行；手动重跑用 `gh workflow run release.yml --ref <tag>`。
  网页的 Run workflow 面板只能选分支，选不了 tag。
- 版本号一律从 `github.ref_name` 现算，没有手工输入；非 tag 的 ref 在 guard 第一步失败。
- 手动触发执行的是「所选 tag 上的工作流文件」，改流程要生效必须先出新 tag。

**工具链与动作**

- Rust 固定 1.97.1，`uses: dtolnay/rust-toolchain@1.97.1`，分支名即版本。该 action 只声明
  `targets` / `target` / `components`，写 `with: toolchain:` 会被忽略并报 "Unexpected input(s)"；
  升级就是换 `@x.y.z` 标签，两处一起改。
- 其余动作取各仓库当前稳定大版本（checkout v7、cache v6、upload-artifact v7、
  download-artifact v8、action-gh-release v3），UPX 5.3.0。不用 `@stable`，不用日期化镜像标签。

**Windows**

- 两个架构走同一份矩阵，步骤必须一致（清单、调试段、体积上限、UPX、重命名）。
- 两个产物同名 `sparrowpdf.exe`，上传前必须按架构重命名，否则 `download-artifact` 的
  `merge-multiple` 会互相覆盖；发布附件名即重命名后的名字。
- 缓存键带 `matrix.arch`，否则两个矩阵任务抢同一个键。

**macOS**

- 通用二进制由 arm64 与 x86_64 两个目标 `lipo -create` 合并，体积上限 25 MB。
- `MACOSX_DEPLOYMENT_TARGET=11.0` 必须与 Info.plist 的 `LSMinimumSystemVersion` 一致。
- `hdiutil` 的 `UDZO` 只能配 `HFS+`；失败用同名重试，最后 `hdiutil verify`。
- 签名是 ad-hoc，签完必须 `codesign --verify`；对外分发仍需 Developer ID 与公证。

**guard 与发布**

- guard 是所有构建 job 的 `needs`，只做只读校验：tag 格式、与 `Cargo.toml` / `Cargo.lock` /
  `src/version.rs` 一致、`CHANGELOG.md` 有该版本条目、打包资源齐全；目的是让错误在 10 秒内暴露。
- 只有 publish 提权到 `contents: write`，其余 job 保持只读。
- Release 正文取自 `CHANGELOG.md` 对应条目，不用平台自动摘要；附件不全由
  `fail_on_unmatched_files` 拦下。

**免费版限额**

- 并发 20 作业、单作业 6 小时、矩阵 256 作业、缓存 10 GB；本流水线最多 5 个作业、每个 ≤60 分钟。
- 公开仓库的标准 runner 不计费；macOS runner 排队无优先级，排队超时被取消时用 Re-run failed jobs。

### 4.6 代码检查与合并前置（`.github/workflows/ci.yml`）

**触发与范围**

- `pull_request` 指向 `main` 或 `dev` 时运行；`push` 到 `dev` 也运行同一条门禁，避免
  「直推后才发现编译不过」。`concurrency` 按 **PR 号 / 分支** 分组，同一 PR 的新提交取消
  旧提交的运行；另留 `workflow_dispatch` 供排查「本地绿、CI 红」。
  **不要改成按提交 SHA 分组**去省掉「dev 直推 + 发版 PR」的重复运行：后到的运行取消
  同组另一个，被取消的检查运行留在同一提交上，GitHub 计入 `statusCheckRollup` 后判为
  FAILURE，`main` 的必需检查从此是红的，发版 PR 再也合不进去（PR #7 实测，2026-11-16）。
- `push` 侧设 `paths-ignore`：纯文档（`*.md`、`docs/**`、`LICENSE`）与依赖调度配置的提交
  不改变二进制，不重跑质量门。**`pull_request` 侧不设**——跳过会让必需检查永远不出现，
  反而把发版 PR 卡死。
- 只做检查，不构建产物、不发 Release；产物由 `release.yml` 在 tag 上构建。

**作业与步骤**

- 单作业「质量门（Windows）」，内容即 4.4 的第 1~3 条：
  `cargo clippy --release --all-targets --locked -- -D warnings`（含类型检查与警告门）、
  `cargo audit`（`taiki-e/install-action` 装预编译二进制，不现场编译），以及
  `cargo test --release --lib --test integration --locked -- --test-threads=1`。
- 另加一道「直接依赖版本唯一性」：用 `cargo tree --depth 1` 取直接依赖、用 `cargo tree -d`
  取出现多版本的 crate，交集非空即失败（例外见 4.7）。它拦的是「直接依赖停在旧版本、
  新升的传递依赖要新版本」这一类隐性重复——编译能过、体积白涨、两份实现行为还可能不一致。
  用文本输出而不是 `cargo metadata` 的 JSON：本包描述是中文，经 pwsh 解码易乱码。
- 测试串行的原因：多数用例共用目标临时目录与同一个日志文件，并发跑会偶发抢写（见 4.3），
  串行只多花几秒。
- 缓存键 `runner.os + pr-cargo-release + hash(Cargo.lock)`，与发布流水线的键分开，互不覆盖。

**合并前置（状态审核，仓库设置里一次性开启）**

- `Settings → Rules`（经典界面为 `Branches`）为 `main` 开启 **Require status checks to pass
  before merging**，勾选「质量门（Windows）」；同时开启 **Require a pull request before merging**
  与 **Block force pushes**，防止绕过。
- **作业名就是检查名**：`ci.yml` 里 `name: 质量门（Windows）` 一改，分支保护中的旧检查名不再匹配，
  PR 会一直等一个不会再出现的检查、永远无法合并——改名必须同步改设置。
- 检查未通过时不得用管理员权限强行合并；确因基础设施故障（runner 宕机、配额）时，用 Re-run
  failed jobs 重跑，仍失败则先修问题。

**已知边界**

- 不设 `cargo fmt --check` 门禁：仓库尚未全量 rustfmt 干净（当前 14 个文件、50 处），开启会立刻
  全红；等一次性 `style` 提交整理干净后再作为独立检查加入，先加先红没有意义。
- 只跑 Windows：产品主战场是 Windows，macOS 由发布流水线在真实产物上验证。若要覆盖 macOS，新增
  `os` 矩阵后检查名会变成两个，分支保护需一并勾选。

### 4.7 依赖更新（Dependabot）流程

依赖更新由三处协作：`dependabot.yml` 决定"提什么"，`ci.yml` 决定"能不能过"，
`deps-merge.yml` 决定"谁来合"。三者缺一段都会退回人工盯 PR。

**分组与调度**

- 目标分支固定 `dev`，每周一次，同时在开的 PR 上限 2。少开一个 PR 就少一整轮质量门，
  `Cargo.lock` 的冲突面也跟着变小；超出的更新排队，不会丢。
- `hayro-*` 单独成组，`cargo-minor-patch` 收其余次版本与补丁（并用 `exclude-patterns`
  把 hayro 排除，避免一个依赖同时命中两组）。hayro 家族互相锁版本——jbig2 0.4 依赖
  ccitt 0.5——拆成两个 PR 时，锁文件会同时出现 0.4 与 0.5 两份传真解码器。
- 主版本（含 0.x 的次版本）不并组，一次只评估一个。
- `lopdf >= 0.40` 继续 ignore，理由见 1.2；`medpdf` / `medpdf-image` 不 ignore。
- `rebase-strategy: auto`：基分支前进后自动跟进，减少"等人解冲突"。

**更新类型与处置**

| `update-type` | 处置 | 依据 |
| --- | --- | --- |
| `semver-patch` | 自动合并 | 补丁不改变 API。`deps-merge.yml` 打 `deps:auto` 并开启 auto-merge，质量门一变绿由 GitHub 合 |
| `semver-minor` / `semver-major` | 人工评审 | 0.x 的次版本按语义化版本就是破坏性变更（hayro-jbig2 0.3 → 0.4 删掉了 `Image`，hayro-ccitt 0.5 改掉了 `Decoder` 接口）。打 `deps:review`，首次打开时留一条评审清单 |

自动合并的唯一条件是「质量门（Windows）」通过，所以任何编译、clippy、测试、漏洞扫描、
依赖唯一性失败都会把合并停在原地，不会带着红检查进 `dev`。合并方式固定 merge commit
（与 6.6 一致），依赖更新可以整体回滚。

**冲突处理**（成本从低到高，前一步能解决就不要往下走）

1. 什么都不做，等 `rebase-strategy: auto`：多数情况 Dependabot 自己 rebase 完。
2. 评论 `@dependabot rebase` 手动触发一次。
3. `gh pr update-branch <n>`（仓库已开 Allow update branch）：只适用于"落后但无冲突"。
4. 本地接手：`git switch <dependabot 分支> && git merge origin/dev`。`Cargo.lock` 冲突
   **不要手改锁文件**——取任意一侧后跑一次 `cargo metadata --offline > NUL` 让 Cargo
   重新求解，再补 `cargo clippy --release --all-targets -- -D warnings` 与
   `cargo tree -d`，确认没有多版本残留。
5. 以上都不行：关掉该 PR，在 `dev` 上直接写一条 `build(deps)` 提交完成升级。依赖升级
   不依赖 PR 存在，PR 只是记录。

**分支约定**

- `dependabot/*` 由机器人创建维护，不受 6.6「只有 `main` / `dev` 两条长期分支」约束；
  **合并后不删除**，也不要往里面塞无关提交。修复必须随该 PR 一起进 `dev` 时可以追加，
  提交正文要说明原因（PR #6 的 `fix(engine)` 即此例）。
- 不对 `dependabot/*` 做 `push --force`；要重新往返就让 Dependabot 自己 rebase。
- 若发现合并后分支被自动清理（2026-11-16 合并 PR #5 时出现过一次），用
  `git push origin <原提交>:refs/heads/<分支名>` 恢复，保持分支集合可追溯。

**登记例外**

直接依赖出现两个版本时 CI 会失败。确认属上游限制（传递依赖钉在另一条大版本线）而非
疏漏时，在 `ci.yml` 的 `$allowed` 与本节**同时**登记 crate 名与原因，缺一即视为未登记。
当前例外：`weezl`（`gif` / `tiff` 等锁 0.1，本项目用 0.2）、`windows-sys`（直接依赖
0.61，另有更低版本来自传递依赖）。

**仓库设置前置**（一次性，代码替代不了）

- `Settings → General`：开启 **Allow auto-merge**（`deps-merge.yml` 依赖它）与
  **Allow update branch**（冲突处理第 3 步）。
- **不要**开启 Automatically delete head branches：分支集合要保留以便追溯。

---

## 5. 编码约定

1. **错误处理**：对外只用 `ToolError` 变体；禁止 `unwrap()` / `expect()` 处理用户输入、路径与 IO。
2. **日志**：用 `engine::logging::log`，不要 `println!`（Windows 发布版无控制台）。
3. **UI 复用**：输入框用 `theme::text_input` / `password_input` / `number_input`；颜色用 `theme::accent/success/warning/danger/card_fill` 等语义函数（自动适配明暗）。
4. **文案**：中文界面文案不使用 emoji 与生僻符号（系统 CJK 字体缺字形会显示为方框），警示用「注意：」前缀。
5. **注释**：解释「为什么」，不解释「是什么」；模块顶部写职责与不变量。
6. **命名**：类型 `UpperCamelCase`，函数/变量 `snake_case`，常量 `SCREAMING_SNAKE_CASE`；缩写按词处理（`PdfToImage`、`pdf_io`）。
7. **性能**：大文档路径必须可取消、可报告进度；UI 线程内不做解码/光栅化（走 worker 或逐帧预算）。
8. **依赖**：新增 crate 需论证（体积、许可证、维护状态、是否为渲染引入外部进程或系统库），并同步更新本文件 1.2 节。

---

## 6. Git 提交规范

采用 **Conventional Commits 1.0.0**，与 SemVer 联动；**标题与正文统一使用中文**（`type`、`scope` 保留英文关键字）。

### 6.1 类型划分

| type | 含义 | 触发版本变化 |
| --- | --- | --- |
| `feat` | 新增用户可见能力 | minor（0.x 阶段同样升 minor） |
| `fix` | 修复缺陷 | patch |
| `perf` | 性能优化，行为不变 | patch |
| `refactor` | 重构，不改外部行为 | 无 |
| `docs` | 文档、注释、README | 无 |
| `test` | 测试用例与测试夹具 | 无 |
| `build` | 构建脚本、依赖、Cargo 配置、图标资源 | 视情况 patch |
| `ci` | 流水线与自动化配置 | 无 |
| `chore` | 杂务（版本号、忽略规则、依赖锁） | patch（版本号时） |
| `style` | 纯格式调整（不含逻辑） | 无 |
| `revert` | 回滚既有提交 | 视被回滚内容 |

### 6.2 scope 取值（可省略，但同一模块应保持一致）

`batch`、`image`、`merge`、`split`、`rotate`、`pages`、`watermark`、`encrypt`、`pdf2img`、`preview`、`ui`、`save`、`engine`、`worker`、`icon`、`build`、`ci`、`release`、`deps`。

### 6.3 格式

```
<type>(<scope>): <中文摘要>

<中文正文：为什么改、改了什么、影响面>

<脚注>
```

硬规则：

- 摘要 ≤ 50 个汉字，动词开头，不加句号，不使用「更新代码」「优化一下」这类空话。
- 摘要行不写版本号；版本信息由 tag 与 CHANGELOG 承载。
- 正文每行 ≤ 72 字符，用 `-` 分条；说明**动机、取舍与影响面**，不复述 diff。
- 破坏性变更：type 后加 `!`，并在脚注写 `BREAKING CHANGE: <影响与迁移方式>`。

### 6.4 示例

```
feat(batch): 新增批量转换，支持多文件夹与两图一页

- 文件名数字感知排序（1、2、10 而非 1、10、2），无数字的按系统顺序尾部排列
- 每个文件夹合成一个同名 PDF，默认写回文件夹自身，可选统一输出目录
- 两图一页支持 A4 横/竖两种槽位，合并序号改为可勾选

Closes #12
```

```
fix(save): 非管理员运行时输出丢失

根因：默认输出落在源文件同目录，受保护目录（如 Program Files）对中等完整性
进程不可写，写入失败但仅记录到 stderr。

- 新增可写性探测与回退链（文档\SparrowPDF 优先），结果缓存避免每帧探测
- 权限类错误映射为中文提示，并在结果区标注实际保存位置
```

```
refactor(engine)!: 统一落盘入口

BREAKING CHANGE: 工具内部不得再直接 fs::write，必须经 pdf_io::save_document；
自定义工具实现需同步迁移。
```

### 6.5 原子性与禁区

- 一个提交只做一件事；重构与功能分开提交，便于回滚。
- 提交前 `git status` 必须干净可解释，禁止把 `target/`、`.probe/`、编辑器配置混入。
- 不提交大二进制（>2 MB）与密钥、令牌、个人路径。
- 不使用 `--no-verify` 跳过钩子；钩子失败先修问题。
- 已推送的提交不 `--amend`、不 `rebase` 重写；需要修正时用新的 `fix` 提交。
- 提交信息不写「WIP」「临时」「先这样」。

### 6.6 分支管理策略

**只有两条长期分支**：`dev` 开发、`main` 发布。不设 `feat/*`、`fix/*`、`release/*` 等临
时分支——功能、缺陷、文档、构建改动一律直接提交到 `dev`。

```
main  ──●────────────────●────────────●──▶  只放已发布版本，每个 ● 带 tag
         \              /            /
dev       ●──●──●──●───●──●──●──●───●────▶  日常开发，始终可编译
```

| 分支 | 角色 | 推进方式 |
| --- | --- | --- |
| `main` | 发布线：只保存可发布/已发布状态，承载版本 tag | 仅由 `dev` 发版合并推进 |
| `dev` | 开发线：日常改动的唯一落点，始终可编译 | 直接提交，随进度推送 |

规则：

1. **日常开发只提交 `dev`**：禁止直接提交或推送到 `main`。`main` 开启分支保护（禁止直接 push、必须经 PR、状态检查「质量门（Windows）」通过才可合并，见 4.6），任何时刻 `main` 上的状态都能发布。
2. **`dev` 始终可编译**：每次提交前跑完 4.4 的质量门，`dev` 上任何一条提交都应能 `cargo build --release`，否则发版前无法用二分定位回归。
3. **一次提交一个议题**：分支只有两条，原子性靠提交而不是分支保证，功能与格式重排不混在同一条提交里。
4. **发版走 PR**：`dev` → `main` 通过 GitHub PR 合入，评审意见与合并记录留在 PR 上；检查未绿时合并按钮不可用，评审结论与检查结果共同构成准入。合并方式在 PR 上选 **Create a merge commit**（等价 `--no-ff`），在 `main` 上留下一次发布边界。**不要**用 Squash（`dev` 与 `main` 会分叉）或 Rebase（改写已共享历史）。
5. **发布以 tag 为准**：PR 合入后，在**合并后的 `main` 提交**上打 `V主.副.修订.YYYYMMDD`，推送后不得移动或复用；随后把 `main` 回合 `dev`（`git switch dev && git merge --ff-only main`），避免两条线分叉。
6. **不改写共享历史**：`main`、`dev` 均由多人共享，不 `--amend`、不 `rebase`、不 `push --force`；远程同步用 `git pull --rebase`。需要试验性改动时用本地临时分支或 `git worktree`，验证后 squash 回 `dev` 并删除。
7. **本地先对齐再开工**：`git fetch origin` 后确认 `dev` 与 `origin/dev` 无分叉；本地 `main` 不得承载未发布的开发提交。
8. **`dependabot/*` 分支是唯一例外**：由机器人创建维护，合并后保留、不手工删改，处置见 4.7。

常用操作：

```bash
git fetch origin
git switch dev                        # 首次：git switch -c dev origin/dev

# …实现 + 4.4 质量门… → 按 6.3 提交，然后
git push origin dev

# 发版（push / 提 PR / 合并 / 打 tag 均需人工授权，见 7.3）
# ① 网页：Compare & pull request，base = main，head = dev；合并时选 Create a merge commit
#    千万不要删 head 分支（dev 是常驻分支）
# ② 合并后本地取回，并在合并提交上打 tag
git switch main && git pull --ff-only
git tag -a V0.6.1.20261120 -m "<该版本变更摘要>"
git push origin V0.6.1.20261120
git switch dev && git merge --ff-only main
```

---

## 7. 标准开发流程

### 7.1 阶段总览

| 阶段 | 准入条件 | 负责人 | 准出交付物 |
| --- | --- | --- | --- |
| 1 需求 | 问题可复现或价值明确 | 需求提出方 | 需求卡（含验收标准） |
| 2 开发 | 需求卡已确认 | 开发者 / AI 代理 | `dev` 上的实现与规范提交 |
| 3 测试 | 代码可编译 | 开发者自测 + 评审人复测 | 测试结果与已知限制说明 |
| 4 评审 | 质量门全绿 | 评审人（非作者） | 评审结论与批准记录 |
| 5 发布 | 评审通过 | 发布负责人 | tag + Release + CHANGELOG |

### 7.2 各阶段职责与交付物

**1）需求**

- 输入：用户反馈、缺陷复现步骤、功能设想。
- 动作：写清「谁在什么场景遇到什么问题」「验收标准是什么」「不做什么」；技术预研只在必要时进行。
- 交付物：需求卡，须包含：背景 / 目标 / 验收标准 / 影响范围 / 优先级。
- 准出：验收标准可被测试用例直接翻译；范围外内容显式列出。

**2) 开发**

- 输入：需求卡。
- 动作：切到 `dev` → 读上下文（第 0 节）→ 实现 → 自测 → 按第 6 节提交。
- 约束：不得改 `Cargo.toml` / `src/version.rs` 的版本号，也不得改 `CHANGELOG.md` 的版本标题——版本号只在发版提交里动（8.3）。日常提交只往 `[Unreleased]` 追加条目。
- 交付物：可编译的 `dev` 状态、原子提交序列、变更说明（改了什么、为什么、影响哪些模块）。
- 准出：`cargo clippy --release --all-targets -- -D warnings` 零警告，集成测试通过；本地可运行。

**3）测试**

- 输入：`dev` 上待测的提交。
- 动作：补/改集成测试；覆盖边界（空输入、越界页码、损坏文件、加密文件、权限受限目录、超长路径）；手工冒烟关键路径。
- 交付物：测试结论（通过/失败项）、已知限制、复现步骤（失败时）。
- 准出：`cargo test --release --lib --test integration` 全绿；新增行为有对应断言。

**4）评审**

- 输入：`dev` 上待发版的提交 + 测试结论 + GitHub PR（base `main`，head `dev`）。
- 动作：在 PR 上检查分层约束、错误处理、落盘路径、取消与进度、UI 一致性与明暗适配、测试有效性、提交信息规范；意见以 review comment 留痕。
- 交付物：PR review 记录（阻塞项 / 建议项分类）与批准结论。
- 准出：阻塞项清零；作者非评审人（无第二人时由作者自审并显式记录「自审」）；合并方式为 merge commit，符合 6.6。

**5）发布**

- 输入：`dev` 上通过评审、准备发版的提交。
- 动作：确认 `dev` 质量门全绿 → 在 `dev` 上改版本号与 `CHANGELOG.md` 并提交（`chore(release): 版本号 A -> B`）→ 推 `dev` → 在 GitHub 开 PR（base `main`，head `dev`）→ 评审通过后以 **Create a merge commit** 合并（不删 head 分支）→ 本地 `git switch main && git pull --ff-only`，确认版本号三处对齐（`Cargo.toml` + `Cargo.lock` 前三段、`src/version.rs` 日期位）→ 在合并后的 `main` 提交上打 annotated tag `V主.副.修订.YYYYMMDD` → 推送 tag → 流水线构建双平台并创建 Release → 验证产物（可启动、关于页版本号与 tag 一致、图标正确）→ 把 `main` 回合 `dev`。
- 交付物：tag、Release 页面（含变更摘要与产物校验信息）、更新后的 CHANGELOG。
- 准出：双平台产物可下载且可运行；`guard` 校验（tag 格式、版本一致、打包资源齐全）通过。
- 回滚预案：产物异常时删除 Release 并标记 tag 作废，修复后升 patch 版本重新发布；不得移动既有 tag。

### 7.3 AI 代理执行清单

| 阶段 | 代理必须做 | 代理不得做 |
| --- | --- | --- |
| 需求 | 复述验收标准，列出不确定点 | 自行扩大范围、臆造验收条件 |
| 开发 | 读上下文、在 `dev` 上提交、最小改动、分类型提交 | 直接提交或推送到 `main`、顺手重构无关模块、改依赖、改 License |
| 测试 | 运行质量门并粘贴真实输出 | 声称「应该通过」而无执行记录 |
| 评审 | 逐项对照 5 节与 4.4 节自检 | 自行批准自己的改动 |
| 依赖更新 | 按 4.7 分流：补丁等质量门绿后合并；次版本 / 主版本先读上游 CHANGELOG 再决定 | 忽略安全告警、删 `dependabot/*` 分支、对共享分支强推 |
| 发布 | 校验版本号一致性、给出推送与打 tag 命令 | 执行 `git push` / 创建 Release / 移动 tag |

---

## 8. CHANGELOG 规范

### 8.1 文件与来源

- 位置：仓库根目录 `CHANGELOG.md`，格式遵循 **Keep a Changelog 1.1.0**。
- 版本号遵循本文件 8.3 节的五位规则（`V主.副.修订.YYYYMMDD`），不采用 SemVer 作为对外版本号。
- 唯一事实来源是**已合并到 `dev`（未发布）与 `main`（已发布）的提交及 tag**；CHANGELOG 是人工提炼的结果，不由 CI 自动生成。
- 发布流水线按 tag 原样截取对应版本条目（`## [V主.副.修订.YYYYMMDD]` 到下一个二级标题为止），作为 GitHub Release 的更新说明；条目缺了 guard 直接失败，所以每条必须独立可读、不依赖上下文。
- 顺序：新版本在上；未发布内容置于 `[Unreleased]`。

### 8.2 变更分类（固定六类，顺序不变）

| 分类 | 收录内容 |
| --- | --- |
| `Added` 新增 | 新工具、新选项、新平台能力 |
| `Changed` 变更 | 行为调整、默认值变化、交互改动 |
| `Deprecated` 弃用 | 仍可用但计划移除的能力 |
| `Removed` 移除 | 已删除的工具/选项/文件格式支持 |
| `Fixed` 修复 | 缺陷修复、崩溃、错误提示改善 |
| `Security` 安全 | 加密、权限、路径处理相关的加固 |

仅保留有内容的分类，空分类不写标题。

### 8.3 版本号规则

对外版本号为五位：`V主版本.副版本.修订版本.日期`，示例 `V0.4.2.20261104`。

| 位 | 含义 | 调整时机 | 决定人 |
| --- | --- | --- | --- |
| 1 | 版本前缀 `V`（version 缩写） | 固定，不变 | — |
| 2 | 主版本号 | 功能模块有较大变动（新增模块、整体架构变化） | 项目经理 |
| 3 | 副版本号 | 功能有一定增加或变化（如权限控制、自定义视图） | 项目经理 |
| 4 | 修订版本号 | Bug 修复与小改动；可高频发布，修掉一个严重 bug 即可发 | 项目经理 |
| 5 | 日期版本号 | **发版当天**的日期 `YYYYMMDD`；日常提交不改它 | 发版人 |

日期位只认「发版当天」：`release.yml` 的 guard 校验 `src/version.rs` 的 `DATE` 与 tag 第四段**完全相等**，
日常改动顺手改成当天日期反而会在发版时对不上。它同时决定「关于」页展示的版本号，提前改会让
开发中的构建显示一个并不存在的版本，所以只在发版那一次提交里动。

落地方式（三处必须与 tag 对齐，缺一即视为版本漂移）：

- `Cargo.toml` / `Cargo.lock`：只保存前三段且**不带 `V`、不带日期**（Cargo 强制 SemVer 三段，例 `0.4.2`）。
- `src/version.rs`：日期位 `DATE`（8 位）；展示版本由 `version::display()` 拼成 `V0.4.2.20261104`，界面与文档不得自行拼字符串。
- git tag：全串 `V主.副.修订.YYYYMMDD`。发布流水线的 guard 校验格式、前三段与 Cargo 一致、日期位与 `version.rs` 一致，任一不符直接失败。
- 一次发布只对应一个 tag；tag 使用 annotated tag，内容为该版本 CHANGELOG 摘要。
- 破坏性变更在该版本条目首行用「**不兼容**：」标明。

### 8.4 条目格式

```
## [V主.副.修订.YYYYMMDD] - YYYY-MM-DD

### 分类

- **模块**：一句话说明改动（`scope`，可选 `#PR`）
```

硬规则：

1. 每条一行，动宾结构，中文，结尾不加句号。
2. 以「模块」加粗开头，模块名与代码 `scope` 一致（如 `批量转换`、`保存`、`文本水印`）。
3. 面向使用者描述**结果**，不写实现细节（实现细节放提交正文）。
4. 引用关联项：`(#12)` 表示 Issue/PR 编号；作者贡献可写 `@name`。
5. 破坏性变更在分类条目下另起一行缩进 2 空格，以「不兼容：」开头说明迁移方式。

### 8.5 示例

```markdown
# 更新日志

本文件记录轻雀 PDF 的所有重要变更，格式遵循 Keep a Changelog，
版本号遵循语义化版本。

## [Unreleased]

### Added
- **批量转换**：支持指定合并序号（如 `1-2,5-6`），未列出的图片单独成页

## [V0.4.2.20261102] - 2026-11-02

### Added
- **外观**：侧边栏新增「浅色 / 深色」切换，选择持久化并在下次启动沿用

### Fixed
- **界面**：侧边栏底部的隐私说明与外观入口不再被滚动区挤出可视范围

## [V0.4.1.20261101] - 2026-11-01

### Changed
- **保存**：源目录不可写时自动改存「文档\SparrowPDF」，结果区标注实际位置并提供「打开所在文件夹」
- **Windows**：可执行文件声明管理员权限，写受保护目录不再失败；另提供免提权构建变体

### Fixed
- **输出**：写入失败不再静默，错误提示与日志均有明确记录

## [V0.4.0.20261101] - 2026-11-01

### Added
- **页面预览**：旋转、删除、提取、拆分面板支持缩略图预览，可收起展开
- **应用图标**：Windows 嵌入可执行文件资源，macOS 打包 icns，窗口与侧边栏同步更新
- **颜色快捷项**：文本水印提供常用色一键选择，减少取色操作

### Changed
- **PDF 转图片**：默认 DPI 由 150 提升至 300，并提示可用范围

### Removed
- **文本提取**：移除该工具，工具总数调整为 12

### Fixed
- **文本水印**：中文不再输出问号，改用系统 TrueType 字形渲染为透明位图叠加
- **字体解析**：修复 `.ttc` 集合字体表偏移错误导致字形全部丢失

[Unreleased]: https://github.com/zfclark/SparrowPDF/compare/V0.4.2.20261102...HEAD
[V0.4.2.20261102]: https://github.com/zfclark/SparrowPDF/compare/V0.4.1.20261101...V0.4.2.20261102
[V0.4.1.20261101]: https://github.com/zfclark/SparrowPDF/compare/V0.4.0.20261101...V0.4.1.20261101
[V0.4.0.20261101]: https://github.com/zfclark/SparrowPDF/compare/V0.3.0.20261031...V0.4.0.20261101
```

### 8.6 维护流程

1. 开发阶段：如有必要，先在 `[Unreleased]` 下累积条目（可选，避免冲突）。
2. 发布前：发布负责人从合并记录中提炼条目，归类并核对措辞，确认无内部术语泄漏。
3. 发版时：把 `[Unreleased]` 内容归档为 `## [V主.副.修订.YYYYMMDD] - 日期`，同步 `src/version.rs` 的日期位，替换比较链接，提交信息用 `chore(release): 版本号 A -> B`。
4. tag 消息取该版本条目作为摘要，使 `git show V主.副.修订.YYYYMMDD` 与 CHANGELOG 一致。
5. 已发布条目**只增不改**；如需更正，追加新版本条目说明。

---

## 9. 附录

### A. 常用命令速查

```bash
# 质量门
cargo clippy --release --all-targets -- -D warnings
cargo test --release --lib --test integration
cargo audit

# 分支（只有 main / dev 两条，日常在 dev，main 只经 PR 合并）
git fetch origin
git switch dev                                  # 首次：git switch -c dev origin/dev
git push origin dev
# 发版 PR：网页 Compare & pull request（base main / head dev），合并选 Create a merge commit

# 发布构建（CI 同款）
cargo build --release --locked
SPARROW_AS_INVOKER=1 cargo build --release      # Windows 免提权变体

# 版本与标签（五位：V主.副.修订.YYYYMMDD）
# 改版本前三段：Cargo.toml 的 version 与 Cargo.lock 同步（cargo check 自动写锁文件）
# 改日期位：src/version.rs 的 DATE，改成发布当天
cargo check --release                            # 同步 Cargo.lock 中的版本号
git switch main && git pull --ff-only             # PR 合并完成后取回 main
git tag -a V0.4.2.20261104 -m "<该版本变更摘要>"
git push origin V0.4.2.20261104                  # tag 必须在 main 的合并提交上
git switch dev && git merge --ff-only main       # 回合，避免两条线分叉

# 日志与偏好定位（排查用）
%LOCALAPPDATA%\sparrowpdf\sparrowpdf.log
%LOCALAPPDATA%\sparrowpdf\ui.pref
```

### B. 新增/修改工具的改动清单

1. `src/model.rs`：`ToolId` 变体、`ToolId::ALL`、`group()`、`label()`、`description()`、`accepted_extensions()`、`max_inputs()`、`is_multi_output()`
2. `src/tools/mod.rs`：`ToolOptions` 变体、默认选项、`tool_for()`、`pub mod` 声明
3. `src/tools/<tool>.rs`：实现 `Tool`，遵守 3.3 的落盘与自校验
4. `src/ui/panels/<tool>.rs` + `panels/mod.rs` 注册
5. `src/app.rs`：`show_panel()` 分支；若需特殊输出规则，补 `resolve_output()` 与 `output_suffix()`
6. `tests/integration.rs`：新增用例；更新工具总数断言（`tool_ids_are_unique_and_complete`）
7. `README.md` 功能表、`CHANGELOG.md`、必要时本文件第 3 节

### C. 禁止事项（硬性）

1. 引入网络调用、遥测、自动更新。
2. 绕过 `pdf_io` 直接落盘，或吞掉写入错误。
3. 在 `ui/` 层引用 PDF/图片库类型。
4. 提交 `target/`、`.probe/`、`.workbuddy/`、密钥与个人绝对路径。
5. 直接向 `main` 提交或推送（发版合并除外）、对 `main` / `dev` 强制推送、移动已发布 tag、改写已共享历史。
6. 未经授权执行 `git push`、创建 Release、修改仓库设置。
7. 用 emoji 充当状态图标（字体缺字形会显示为方框）。

### D. 合并请求（PR）

模板在 `.github/pull_request_template.md`，开 PR 时自动带出，含变更内容 / 关联需求 / 影响面 /
验证方式 / 回滚方案五段与质量门勾选项；模板改动直接改该文件，本节不再复制一份。

合入条件（缺一不可）：状态检查「质量门（Windows）」通过（4.6）+ 评审意见已处理 + 合并方式为
merge commit。发版 PR 的 base 是 `main`、head 是 `dev`，且不删 head 分支。
