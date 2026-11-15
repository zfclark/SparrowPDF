# 轻雀 PDF（SparrowPDF）

> PDF 的事，本地办完。

面向日常 PDF 处理的桌面工具集：合并、拆分、旋转、删页、加水印、加密、转图片。
全部在本机完成 —— 不联网、不上传、无遥测、无需账号。

![platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS-blue)
![rust](https://img.shields.io/badge/rust-1.95%2B-orange)
![license](https://img.shields.io/badge/license-MIT-blue)

- **免安装**：Windows 单个 exe，macOS 一个 `.app`，不常驻后台。
- **纯本地**：没有网络代码、没有崩溃上报、没有自动更新检查。
- **12 项工具**：从扫描件成册到加密交付，覆盖日常材料处理的全流程。

## 下载

| 系统 | 文件 | 说明 |
| --- | --- | --- |
| Windows 10 / 11（x64） | `SparrowPDF-windows-x64.exe` | 免安装，打开即用 |
| Windows 10 / 11 on ARM | `SparrowPDF-windows-arm64.exe` | 原生 ARM64 构建 |
| macOS 11+（Intel / Apple Silicon） | `SparrowPDF-macos-universal.dmg` | 通用二进制，双架构共用 |

均从 [Releases](https://github.com/zfclark/SparrowPDF/releases/latest) 页下载。ARM 版适用于
骁龙 8cx / X 系列等 Windows on ARM 设备，系统要求 Windows 10 及以上。

Windows 版默认请求管理员权限，原因见 [Windows 的提权](#windows-的提权)。

macOS 包为 ad-hoc 签名、未经公证，首次打开可能提示「无法验证开发者」或「已损坏」。确认来源后
在终端执行一次解除隔离：

```bash
xattr -dr com.apple.quarantine /Applications/SparrowPDF.app
```

## 功能一览

| 分组 | 工具 | 说明 |
| --- | --- | --- |
| 创建 | 图片合并 PDF | 多张 JPG / PNG 按顺序合成单个 PDF；竖拍照片自动校正方向 |
| 创建 | 批量转换 | 每个文件夹生成一个 PDF；按材料选模板（标准、身份证、入团志愿书、A5 小册子），文件名按数值排序 |
| 合并 | PDF 合并 | 拖拽调整顺序后合成单个 PDF |
| 拆分 | PDF 拆分 | 按页码范围、每 N 页、或一页一个文件 |
| 页面 | 页面旋转 | 每次 90°，可连续操作，按累计角度生效 |
| 页面 | 删除页面 | 指定页码删除 |
| 页面 | 页面重排 | 拖拽排序，实时预览输出页序 |
| 页面 | 提取页面 | 选中的页单独存为新文件 |
| 水印 | 文本水印 | 平铺或居中，可调字号、透明度、角度，使用系统中文字体 |
| 水印 | 图片水印 | 支持缩放、透明度与叠加位置 |
| 安全 | 加密 / 解密 | AES-256，可分别限制打印 / 复制 / 修改；亦可移除密码 |
| 导出 | PDF 转图片 | 一次可转多个 PDF，每页导出 PNG / JPEG，36–600 DPI（默认 300） |

## 快速上手

1. 在左侧选工具，中间显示该工具的参数。
2. 添加文件（或整个文件夹）；需要顺序的工具可直接拖拽调整。
3. 确认「输出」位置，默认与源文件同目录。
4. 点底部「执行」，进度与结果都在同一条状态栏里。

## 使用要点

**同名文件不覆盖。** 程序先规划本次要写的全部文件，检测到重名时暂停并询问：覆盖、另存为新文件
（追加 `-2`、`-3`）或取消。批量任务一次性列出全部冲突，单次作答即可，可勾选「本次后续均执行
此操作」。

**输出位置优先采用你指定的路径。** 未指定时写入源文件同目录；目录不可写（如 `Program Files`）
时自动改存至「文档\SparrowPDF」，并在结果卡片中标注实际路径。

**外观跟随系统。** 侧边栏底部可切换浅色 / 深色，选择会持久化，下次启动沿用。

## 平台说明

### Windows 的提权

默认版本启动时请求 UAC 提权，以便直接写入 `Program Files` 等受保护目录。代价是系统禁止向提权
窗口拖放文件。需要保留拖放时用免提权变体：

```bash
SPARROW_AS_INVOKER=1 cargo build --release
```

该变体写受保护目录会失败，同样自动改存至「文档\SparrowPDF」。

### macOS 的权限

无提权机制：权限由 POSIX/ACL 与 TCC 决定，`.app` 以当前用户身份运行。

## 从源码开发

最低 Rust 1.95（edition 2021）。Windows 另需 MSVC 与 Windows SDK；若 `cl.exe` 报
`C1083 无法打开 stddef.h`，说明当前终端缺 `INCLUDE` / `LIB`，改用 VS 开发者命令提示符。

```bash
cargo run                                                   # 开发期运行，改完即可看效果
cargo clippy --release --all-targets -- -D warnings        # 类型检查 + 警告门（CI 同款）
cargo test --release --lib --test integration               # 单元 + 集成测试
cargo build --release --locked                              # 本地发布构建（与 CI 一致）
```

`check` 与 `test` 一律带 `--release`：debug 构建会触发 `libz-sys` 的 `D8050`，渲染路径也慢到
无法判断性能。

工程约束、分层、测试策略与提交规范以 [AGENTS.md](./AGENTS.md) 为准；实现细节按主题在
[docs/](./docs/) 里查，动手前先读对应主题。

### 贡献流程

只有两条长期分支：`dev` 开发、`main` 发布；发版时把 `dev` 合并进 `main`，一次合并提交就是一个版本。

```bash
git switch dev                          # 首次：git switch -c dev origin/dev

# 改代码 → cargo clippy --release --all-targets -- -D warnings → cargo test --release --lib --test integration

git commit -m "fix(merge): 合并时按用户拖拽结果写入页序"
git push origin dev
```

- **提交信息**：Conventional Commits，标题与正文用中文，`type` 与 `scope` 用英文
  （`feat` / `fix` / `perf` / `refactor` / `docs` / `test` / `build` / `ci` / `chore` / `style`）。
  一次提交只做一件事，便于单条回滚。
- **质量门**：提交前 `cargo clippy --release --all-targets -- -D warnings` 零警告、
  `cargo test --release --lib --test integration` 全通过；改动涉及界面时附一次实际启动确认。
  同一组检查（外加 `cargo audit` 依赖漏洞扫描）会在 PR 与 `dev` 推送时自动跑（状态检查
  「质量门（Windows）」），未通过不能合并。
- **变更记录**：用户可见的行为变化在 `CHANGELOG.md` 的 `[Unreleased]` 下加一条；测试与文档改动
  不必。
- **分支与权限**：没有写权限时先 fork，从 `dev` 切出改动并向本仓库的 `dev` 提 PR；有写权限直接
  提交 `dev`。不要直接提交 `main` —— 发版由维护者开 `dev` → `main` 的 PR，评审通过后用 merge
  commit 合并（不用 squash），再在 `main` 上打 `V主.副.修订.YYYYMMDD` tag；贡献者不要改
  `Cargo.toml` 里的版本号。
- **不要提交**：`target/`、`.probe/` 与调试素材；也不要引入网络请求、遥测与自动更新。

## 已知限制

- **内置渲染器**：「PDF 转图片」覆盖常见文档；shading、Type3 字体、复杂混合模式等暂不支持，
  遇到这类内容会跳过对应部分，页面其余内容照常输出。排版复杂的文档建议对照专业阅读器复核。
- **字体**：未内嵌字体的 PDF 以系统字体替代，字形与字宽可能与专业渲染器存在差异。
- **加密**：仅支持 AES-256；不支持数字签名与表单填写。
- **损坏文件**：文件损坏或密码错误时直接报错，不产出空白文件。

渲染器支持范围、实现细节与已知边界见 [docs/](./docs/)；协作者规范见 [AGENTS.md](./AGENTS.md)。

## 许可证

本项目使用 [MIT](./LICENSE) 许可证授权。

第三方组件：

- JPEG 编码：[`jpeg-encoder`](https://github.com/vstroebel/jpeg-encoder)（MIT / Apache-2.0），
  其熵编码实现部分源自 Independent JPEG Group 的工作（IJG 许可证）。
- 图像缩放：[`fast_image_resize`](https://github.com/cykooz/fast_image_resize)（MIT / Apache-2.0）。
