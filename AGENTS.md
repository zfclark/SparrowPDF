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
写在 `docs/`，需要时按主题查阅，不必通读。

执行原则：

- **先读后写**：不得在未读源码的情况下凭猜测改接口。
- **最小改动**：一次任务只解决一个议题，不夹带格式重排、无关重命名。
- **验证前置**：改完必须跑第 4.4 节的质量门，输出真实结果，不臆测「应该能过」。
- **不做外部动作**：`git push`、`git tag` 推送、创建 Release、修改仓库设置等一律需人工明确授权。
- **不确定就问**：涉及依赖增删、公开接口破坏、License 变更时停下确认。

---

## 1. 项目概览

### 1.1 定位与硬性约束

| 项目   | 说明                                           |
| ---- | -------------------------------------------- |
| 产品名  | 轻雀 PDF / SparrowPDF                          |
| 形态   | 单机桌面应用（Windows / macOS），单文件可执行               |
| 语言   | Rust，edition 2021，MSRV 1.95（CI 固定 1.97.1）    |
| 界面   | egui / eframe 0.37，Glow 后端，无 WebView         |
| 数据流  | **纯本地**：不联网、不上传、无遥测、无账号体系                    |
| 工具数  | 12 项，分组：生成 2 / 结构操作 6 / 页面编辑 2 / 安全 1 / 转换 1 |
| 安装形态 | 绿色 exe（Windows x64 / arm64）、`.app` 打包为 DMG（macOS 通用版） |

破坏性约束（任何改动都不得违反）：

1. 不引入网络请求、埋点、崩溃上报、自动更新检查。
2. 不改动 `LICENSE`（MIT，版权人 霖海）。
3. 不提交二进制产物、构建缓存、本地调试素材。
4. 新增依赖仍须过第 5.8 节的论证（体积、许可证、维护状态），并在 1.2 节登记。

### 1.2 技术栈

| 依赖                                    | 用途                             |
| ------------------------------------- | ------------------------------ |
| `eframe` / `egui` 0.37                | 窗口与 immediate-mode UI          |
| `egui_dnd` 0.18                       | 页面/文件拖拽排序                      |
| `rfd` 0.17                            | 系统文件/文件夹选择对话框                  |
| `lopdf` 0.39                          | PDF 对象模型、读写、加密                 |
| `medpdf` 0.9.2 / `medpdf-image` 0.2.2 | 文本度量、图片编解码与嵌入                  |
| `kamadak-exif` 0.6                    | JPEG 的 EXIF 方向读取               |
| `image` 0.25（jpeg, png）               | 图片解码、PNG 编码                    |
| `allsorts` 0.16（flate2_rust）          | 系统 TrueType 字体解析（用于内置光栅器与中文水印） |
| `weezl` 0.2                            | LZW 解码（图像 `/LZWDecode`）           |
| `hayro-ccitt` 0.4                      | CCITT G3/G4 传真解码（`/CCITTFaxDecode`） |
| `hayro-jbig2` 0.3（std）                | JBIG2 解码（`/JBIG2Decode`），含共享全局段 |
| `flate2` 1（rust_backend）              | zlib 解压（图像 `/FlateDecode` 与预测器流水线） |
| `winresource` 0.1（build-dep）          | 向 Windows exe 注入图标与清单          |

---

## 2. 目录结构与职责

### 2.1 顶层结构

```
SparrowPDF/
├── Cargo.toml            # 包元数据、依赖、release profile（体积/性能开关）
├── Cargo.lock            # 锁定依赖，发布构建使用 --locked
├── build.rs              # Windows 资源：app.ico + 应用清单（提权/长路径/DPI）
├── assets/               # 图标素材（icns / ico / png），不参与运行时解码逻辑
├── src/                  # 全部 Rust 源码（见 2.2）
├── tests/integration.rs  # 端到端集成测试（不启动 GUI，直接调 Tool）
├── README.md             # 面向用户的功能与安装说明
├── AGENTS.md             # 本文件：面向协作者的工程规范
├── docs/                 # 技术实现文档（分层、工具、渲染、界面），改代码前读对应主题
└── .github/workflows/    # 发布流水线（tag 触发，产出 Windows x64 / arm64 与 macOS 通用版）
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
   │    ├── worker.rs   后台线程 + catch_unwind + 取消
   │    ├── validate.rs 产物断言（页数、非空文本等）
   │    ├── render.rs   PDF 页面软件光栅化（缩略图与 PDF 转图片）
   │    ├── raster.rs   像素缓冲与绘制原语
   │    ├── ttf.rs      TrueType/OpenType 解析（支持 .ttc 集合）
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

| 文件                     | 职责                          | 修改风险        |
| ---------------------- | --------------------------- | ----------- |
| `src/model.rs`         | 工具注册表与错误模型；新增工具必须在此登记       | 高：影响全部面板与测试 |
| `src/tools/mod.rs`     | `Tool` 契约、选项枚举、进度/取消通道、工厂函数 | 高           |
| `src/engine/pdf_io.rs` | 唯一的落盘入口（原子写）、密码打开、可写性探测     | 高：写坏即数据丢失   |
| `src/engine/worker.rs` | 线程生命周期、panic 捕获、取消传播        | 中           |
| `src/app.rs`           | 输出路径解析与回退、任务编排、结果状态机        | 中高          |
| `src/ui/theme.rs`      | 全部视觉令牌与通用控件                 | 中：改动影响所有面板  |
| `build.rs`             | Windows 清单与图标；提权策略          | 中：可令程序无法启动  |

---

## 3. 核心功能与契约

### 3.1 工具清单

| 分组   | 工具        | 输入            | 产物                   |
| ---- | --------- | ------------- | -------------------- |
| 生成   | 图片合并为 PDF | 多张 JPG/PNG    | 单个 PDF               |
| 生成   | 批量转换      | 多个文件夹         | 每个文件夹一个 PDF          |
| 结构操作 | PDF 合并    | ≥2 个 PDF      | 单个 PDF               |
| 结构操作 | PDF 拆分    | 1 个 PDF       | 按页码范围 / 每 N 页 / 逐页输出 |
| 结构操作 | 页面旋转      | 1 个 PDF       | 旋转后的 PDF             |
| 结构操作 | 删除页面      | 1 个 PDF       | 删页后的 PDF             |
| 结构操作 | 页面重排      | 1 个 PDF       | 重排后的 PDF             |
| 结构操作 | 提取页面      | 1 个 PDF       | 抽取页组成的新 PDF          |
| 页面编辑 | 文本水印      | 1 个 PDF       | 叠加文字水印的 PDF          |
| 页面编辑 | 图片水印      | 1 个 PDF + 1 图 | 叠加图片水印的 PDF          |
| 安全   | 加密 / 解密   | 1 个 PDF       | AES-256 加密或移除密码      |
| 转换   | PDF 转图片   | 1 个 PDF       | 每页 PNG/JPEG          |

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
3. **统一经 `pdf_io::save_document`（或 `write_file`）落盘，并把 `req.conflict` 一并传入**：两者在写之前会再复核一次目标是否存在（`ConflictPolicy::Report` 时直接返回 `OutputExists`），先写同目录临时文件再 `rename`，避免半成品；权限错误由 `write_error_detail` 映射为中文提示。规划与落盘之间可能隔几分钟的渲染，这次复核是防静默覆盖的第二道闸。
4. 落盘后**重新打开产物断言**（`validate::assert_page_count` 等）；断言失败要删除产物并返回 `ValidationFailed`。
5. 不得在工具内部直接 `fs::write` 或 `File::create` 绕过第 3 步。

### 3.4 实现细节

具体实现按主题写在 `docs/`，这里不再展开：

| 要改的东西           | 先读                       |
| -------------------- | -------------------------- |
| 某个工具的处理逻辑   | `docs/tools.md`            |
| 渲染、导出质量       | `docs/rendering.md`        |
| 界面、布局、任务状态 | `docs/ui.md`               |
| 分层、新增模块、平台 | `docs/architecture.md`     |

读法：先看本节的 3.2 / 3.3 契约，再读对应文档；每篇末尾的「已知缺口」是既有边界，
不要当成回归缺陷去修。

---

## 4. 构建与验证

### 4.1 常用命令

```bash
cargo check --release --all-targets      # 快速类型检查（含测试）
cargo build --release --locked           # 发布构建（CI 同款）
cargo test --release --lib --test integration
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
| 集成测试 | `tests/integration.rs`（44 个用例） | 工具端到端：页码解析、合并、旋转、删页、加密往返、拆分、中文水印位图断言、日志追加、图片自动旋转、批量排序/两图一页/非法序号；同名文件三种策略（Report/Overwrite/Unique）与落盘前复核、LZW / CCITT / JBIG2（含有效码流）/ PNG 预测器 / 原始采样图像渲染、CID 文本经 `/ToUnicode` 取字、内嵌 `/FontFile2` 无 ToUnicode 出字 |
| 单元测试 | 模块内 `#[cfg(test)]` | 纯函数（码表、偏好文件读写等） |

规则：

- 新增或修改工具行为 → **必须**补一条集成测试，断言产物结构（页数、MediaBox、矩阵、XObject 等），不只断言「执行成功」。
- 测试的临时目录走 `CARGO_TARGET_TMPDIR`（工作区内），不写系统 temp，避免受限环境失败。注意它只存在于**编译期**环境，必须用 `option_env!` 读取，运行时 `std::env::var` 永远取不到（会静默退回系统 temp）。
- 测试夹具由测试自行生成（`make_pdf` / `make_image`），不依赖仓库内二进制。

### 4.4 提交前质量门（必须全绿）

1. `cargo check --release --all-targets` 无警告（`warning: unused` 视为失败）。
2. `cargo test --release --lib --test integration` 全通过。
3. 若改动涉及 UI：至少一次实际启动（`cargo build --release` 后运行）确认无渲染异常、无布局裁切。
4. 若改动版本号：`Cargo.toml` 与 `Cargo.lock` 同步，且与将要打的 tag 一致。
5. 若改动发布流水线：说明验证方式（无 actionlint 时至少核对结构与关键参数）。

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

采用 **主干开发 + 短生命周期分支**：

| 分支 | 命名 | 来源 | 合并去向 | 存活期 |
| --- | --- | --- | --- | --- |
| 主干 | `main` | — | — | 常驻，始终可发布 |
| 功能 | `feat/<scope>-<简述>` | `main` | `main`（squash） | ≤ 3 天 |
| 缺陷 | `fix/<scope>-<简述>` | `main` | `main`（squash） | ≤ 1 天 |
| 维护 | `ci/<简述>`、`docs/<简述>`、`build/<简述>` | `main` | `main`（squash） | ≤ 1 天 |
| 发布 | `release/<主.副.修订>` | `main` | `main` + `V主.副.修订.YYYYMMDD` tag | 发布窗口内 |
| 紧急修复 | `hotfix/<主.副.修订>` | 线上 tag | `main` + 新 tag | ≤ 1 天 |

规则：

1. `main` 受保护：禁止直接推送，必须经评审合并；历史保持线性（squash 或 rebase-and-merge，不产生无意义 merge commit）。
2. 分支名全小写、用连字符分隔，不含中文与空格。
3. 发布以 **tag 为准**：`V主.副.修订.YYYYMMDD`（如 `V0.4.2.20261104`）打在 `main` 上，tag 一旦推送不得移动或复用。
4. 禁止对 `main` 强制推送；`hotfix` 必须同时回合 `main`，避免双线分叉。
5. 远程同步用 `git pull --rebase`，本地分支可随意整理，已共享的分支不改写。

---

## 7. 标准开发流程

### 7.1 阶段总览

| 阶段 | 准入条件 | 负责人 | 准出交付物 |
| --- | --- | --- | --- |
| 1 需求 | 问题可复现或价值明确 | 需求提出方 | 需求卡（含验收标准） |
| 2 开发 | 需求卡已确认 | 开发者 / AI 代理 | 分支上的实现与规范提交 |
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
- 动作：建分支 → 读上下文（第 0 节）→ 实现 → 自测 → 按第 6 节提交。
- 交付物：可编译分支、原子提交序列、变更说明（改了什么、为什么、影响哪些模块）。
- 准出：`cargo check --release --all-targets` 无警告；本地可运行。

**3）测试**

- 输入：开发分支。
- 动作：补/改集成测试；覆盖边界（空输入、越界页码、损坏文件、加密文件、权限受限目录、超长路径）；手工冒烟关键路径。
- 交付物：测试结论（通过/失败项）、已知限制、复现步骤（失败时）。
- 准出：`cargo test --release --lib --test integration` 全绿；新增行为有对应断言。

**4）评审**

- 输入：待合并分支 + 测试结论。
- 动作：检查分层约束、错误处理、落盘路径、取消与进度、UI 一致性与明暗适配、测试有效性、提交信息规范。
- 交付物：评审意见（阻塞项 / 建议项分类）与最终结论。
- 准出：阻塞项清零；作者非评审人；合并方式符合 6.6。

**5）发布**

- 输入：`main` 上通过评审的提交。
- 动作：确认版本号（`Cargo.toml` + `Cargo.lock` 的前三段、`src/version.rs` 的日期位，三处与 tag 对齐）→ 更新 `CHANGELOG.md` → 打 annotated tag `V主.副.修订.YYYYMMDD` → 推送分支与 tag → 流水线构建双平台并创建 Release → 验证产物（可启动、关于页版本号与 tag 一致、图标正确）。
- 交付物：tag、Release 页面（含变更摘要与产物校验信息）、更新后的 CHANGELOG。
- 准出：双平台产物可下载且可运行；`guard` 校验（tag 格式、版本一致、打包资源齐全）通过。
- 回滚预案：产物异常时删除 Release 并标记 tag 作废，修复后升 patch 版本重新发布；不得移动既有 tag。

### 7.3 AI 代理执行清单

| 阶段 | 代理必须做 | 代理不得做 |
| --- | --- | --- |
| 需求 | 复述验收标准，列出不确定点 | 自行扩大范围、臆造验收条件 |
| 开发 | 读上下文、最小改动、分类型提交 | 顺手重构无关模块、改依赖、改 License |
| 测试 | 运行质量门并粘贴真实输出 | 声称「应该通过」而无执行记录 |
| 评审 | 逐项对照 5 节与 4.4 节自检 | 自行批准自己的改动 |
| 发布 | 校验版本号一致性、给出推送命令 | 执行 `git push` / 创建 Release / 移动 tag |

---

## 8. CHANGELOG 规范

### 8.1 文件与来源

- 位置：仓库根目录 `CHANGELOG.md`，格式遵循 **Keep a Changelog 1.1.0**。
- 版本号遵循本文件 8.3 节的五位规则（`V主.副.修订.YYYYMMDD`），不采用 SemVer 作为对外版本号。
- 唯一事实来源是**已合并到 `main` 的提交与 tag**；CHANGELOG 是人工提炼的结果，不由 CI 自动生成。
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
| 5 | 日期版本号 | 改动当天的日期 `YYYYMMDD`，每天改动都要更新 | 开发人员 |

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
cargo check --release --all-targets
cargo test --release --lib --test integration

# 发布构建（CI 同款）
cargo build --release --locked
SPARROW_AS_INVOKER=1 cargo build --release      # Windows 免提权变体

# 版本与标签（五位：V主.副.修订.YYYYMMDD）
# 改版本前三段：Cargo.toml 的 version 与 Cargo.lock 同步（cargo check 自动写锁文件）
# 改日期位：src/version.rs 的 DATE，改成发布当天
cargo check --release                            # 同步 Cargo.lock 中的版本号
git tag -a V0.4.2.20261104 -m "<该版本变更摘要>"
git push origin main
git push origin V0.4.2.20261104

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
5. 对 `main` 强制推送、移动已发布 tag、改写已共享历史。
6. 未经授权执行 `git push`、创建 Release、修改仓库设置。
7. 用 emoji 充当状态图标（字体缺字形会显示为方框）。

### D. 合并请求（PR）模板

```markdown
## 变更内容
<!-- 一句话说明这个 PR 解决什么问题 -->

## 关联需求
Closes #<编号>

## 影响面
- 涉及模块：
- 用户可见变化：
- 兼容性：无 / 有（说明）

## 验证方式
- [ ] `cargo check --release --all-targets` 无警告
- [ ] `cargo test --release --lib --test integration` 全通过
- [ ] 手动冒烟：（步骤）
- [ ] 涉及 UI 时附截图

## 回滚方案
<!-- 撤销方式：回滚提交 / 关闭开关 / 恢复默认值 -->
```
