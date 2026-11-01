# 轻雀 PDF（SparrowPDF）

> PDF 的事，本地办完。

轻雀 PDF 是一款纯 Rust 实现的本地 PDF 工具箱：无联网、无上传、无遥测，所有处理都在你自己的电脑上完成。

![platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS-blue) ![rust](https://img.shields.io/badge/rust-1.95%2B-orange)

## 功能（12 项）

| 分组 | 工具 | 说明 |
| --- | --- | --- |
| 创建 | 图片合并 PDF | JPEG 走 DCTDecode 直通（不重编码），EXIF 方向自动纠正；PNG 以 FlateDecode 内嵌 |
| 合并 | PDF 合并 | 多文件拖拽排序后按序合并 |
| 拆分 | PDF 拆分 | 按页码范围 / 每 N 页 / 逐页三种模式 |
| 页面 | 页面旋转 | 以 90° 增量旋转，多次操作按累计语义合并 |
| 页面 | 删除页面 | 按页码范围删除 |
| 页面 | 页面重排 | 拖拽排序，输出按新顺序重组 |
| 页面 | 提取页面 | 把选中的页抽取为新文件 |
| 水印 | 文本水印 | 平铺或居中；系统 CJK 字体；支持透明度与旋转角 |
| 水印 | 图片水印 | 缩放适配、透明度、覆盖层 |
| 安全 | 加密 / 解密 | AES-256，可组合打印 / 复制 / 修改权限 |
| 导出 | PDF 转图片 | 纯 Rust 软件光栅化，PNG / JPEG，36–600 DPI |
| 导出 | 文本提取 | 按页提取文本到 .txt |

## 构建

```bash
cargo build --release
```

- 最低 Rust 版本：1.95（edition 2021）。
- 无需任何系统级 C/C++ 依赖（见下方「依赖说明」）。

### 体积

Release profile 启用 `panic=abort + codegen-units=1 + lto + opt-level=z + strip`，Windows x64 单文件 exe 约 5 MB（UPX 压缩前），远低于 20 MB 上限。macOS Universal（arm64 + x86_64）DMG 由 CI 产出。

## UI 语言与中文字体

界面为简体中文。为遵守体积红线，程序**不内嵌 CJK 字体**：启动时会按顺序探测系统字体（微软雅黑 / 等线 / 黑体 / 宋体 / PingFang / Noto Sans CJK / DejaVu Sans 等），命中后以运行时回退字体注入 egui。若系统一个 CJK 字体都没有（极罕见），中文界面文字会显示为方块（tofu）——功能不受影响。

## 纯 Rust 光栅化的能力边界

「PDF 转图片」没有使用 PDFium / MuPDF / Poppler 等任何 C/C++ 渲染库，而是自研软件光栅化器（`src/engine/render.rs` + `raster.rs` + `ttf.rs`）。它覆盖常见算符与对象：

- 路径：`m l c v y h re` + 填充（非零环绕 / 奇偶）/ 描边 / `B b` 系列；
- 色彩：灰度、RGB、CMYK，`gs` 中的 ca/CA 透明度；
- 图像：DCTDecode（JPEG）与 FlateDecode 的 DeviceGray/RGB/CMYK，Form XObject 递归；
- 文本：`BT/ET` 块、Tf/Tm/Td/TD/T*/TL/Tc/Tw/Tz/Tj/'/"/TJ，TrueType 字形轮廓填充（含复合字形、fmt4/fmt12 cmap）；
- 页面：MediaBox 与 /Rotate 沿页树 Parent 链继承。

**不支持的算符会被跳过**（如 shading pattern、Type3 字体、复杂 blend mode）；当输出图片无法辨认时工具会显式报错，不会静默产出空白图。文字渲染使用系统字体近似匹配 PDF 内嵌字体，字形可能与专业渲染器存在细微差异。这是纯 Rust 方案在体积与依赖约束下的有意取舍。

## 依赖说明

| 依赖 | 用途 |
| --- | --- |
| eframe / egui 0.37 | GUI（glow 渲染后端） |
| egui_dnd 0.18 | 列表拖拽排序 |
| rfd 0.17 | 原生文件对话框 |
| lopdf 0.39 | PDF 对象模型读写 |
| medpdf 0.9.2 / medpdf-image 0.2.2 | 页面操作 / JPEG 直通内嵌 |
| kamadak-exif 0.6 | EXIF 方向读取 |
| image 0.25 | PNG/JPEG 编解码 |

全部依赖均为 Rust 实现。一个已知例外：`medpdf` 在其自身清单中以默认特性引用 `allsorts`，特性合并后会把 `flate2 → libz-sys`（静态链接的 C zlib）带入依赖图。本仓库已在顶层声明 `allsorts default-features=false, features=["flate2_rust"]` 表达纯 Rust 意图；若未来 `medpdf` 改用无默认特性的 `allsorts`，该 C 依赖会自动消失。

## 已知限制

- **中文水印后文本不可提取**：为保证体积，水印使用的系统字体子集化后丢失 ToUnicode 映射，含水印页的该段文字将无法被文本提取工具还原（页面显示正常）。此为 PDF 子集化的固有代价。
- **加密 PDF 的兼容性**：产物按 PDF 1.6+ 的 AES-256（R6）写入，旧阅读器（Acrobat 8 及更早）无法打开。
- **PDF 转图片**：见上文「能力边界」。
- Windows 端为本地开发目标；macOS Universal 构建仅由 CI（GitHub Actions）验证。

## CI

`.github/workflows/release.yml` 在推送 `vX.Y.Z` 标签时触发：

1. **guard**：校验标签格式与 Cargo.toml 版本一致、tag 位于 main 可达提交上；
2. **build-windows**：编译 x64 release，断言体积 < 20 MB（UPX 前）、无可执行内嵌调试段，UPX 压缩（失败不阻塞）；
3. **build-macos**：单 runner 交叉编译 arm64 + x86_64 两切片，`lipo` 合并为 Universal，手工组装 SparrowPDF.app（Info.plist + ad-hoc 签名）并打成 DMG；
4. **publish**：汇总产物并创建 GitHub Release。

## License

MIT
