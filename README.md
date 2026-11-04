# 轻雀 PDF（SparrowPDF）

> PDF 的事，本地办完。

轻雀 PDF 是一款基于 Rust 实现的本地 PDF 工具箱：无联网、无上传、无遥测，所有处理都在你自己的电脑上完成。

![platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS-blue) ![rust](https://img.shields.io/badge/rust-1.95%2B-orange)

## 功能（12 项）

| 分组 | 工具         | 说明                                                               |
| ---- | ------------ | ------------------------------------------------------------------ |
| 创建 | 图片合并 PDF | JPEG 走 DCTDecode 直通，EXIF 方向自动纠正；PNG 以 FlateDecode 内嵌 |
| 创建 | 批量转换     | 多个文件夹分别合成 PDF，文件名数字感知排序；可两图一页        |
| 合并 | PDF 合并     | 多文件拖拽排序后按序合并                                           |
| 拆分 | PDF 拆分     | 按页码范围 / 每 N 页 / 逐页三种模式                                |
| 页面 | 页面旋转     | 以 90° 增量旋转，多次操作按累计语义合并                            |
| 页面 | 删除页面     | 按页码范围删除                                                     |
| 页面 | 页面重排     | 拖拽排序，实时预览输出页序，按新顺序重组                           |
| 页面 | 提取页面     | 把选中的页抽取为新文件                                             |
| 水印 | 文本水印     | 平铺或居中；系统 CJK 字体；支持透明度与旋转角                      |
| 水印 | 图片水印     | 缩放适配、透明度、覆盖层                                           |
| 安全 | 加密 / 解密  | AES-256，可组合打印 / 复制 / 修改权限                              |
| 导出 | PDF 转图片   | 内置软件光栅化，PNG / JPEG，36–600 DPI（默认 300）                 |

## 界面与外观

- 侧边栏底部有 **「外观：浅色 / 深色」** 切换；点击即时生效，选择写入
  `%LOCALAPPDATA%\sparrowpdf\ui.pref`，下次启动沿用。
- 未手动选择过时跟随操作系统主题（系统切换深色 / 浅色，应用随之变化）。
- 所有颜色（卡片、输入框、提示与状态色）都按当前主题解析，无需重启。
- 侧边栏底部还有 **「关于」** 入口（右侧显示完整版本号），弹窗展示
  基本信息、作者与项目地址；项目地址点击后交由系统默认浏览器打开。

## 保存与同名文件

- 输出文件与已有文件同名时**不会直接覆盖**：程序先规划本次要写的全部文件名，发现同名就停下询问。
- 三种选择：**覆盖**、**另存为新文件**（自动追加 `-2`、`-3` 序号）、**取消**（什么都不做）。
- 批量任务（拆分、批量转换、PDF 转图片）会把冲突一次性汇总，一次回答覆盖整批。
- 勾选「本次后续均执行此操作」后，同一会话内不再重复询问。

## 构建

```bash
cargo build --release
```

- 最低 Rust 版本：1.95（edition 2021）。

### Windows：提权与免提权变体

默认构建的 exe 内嵌清单 `requestedExecutionLevel = requireAdministrator`，
启动时会请求管理员权限——这样输出可以直接写入受保护目录（如
`C:\Program Files`、其他用户的目录）。代价：每次启动弹一次 UAC，
且系统禁止把文件从资源管理器拖入提权进程（拖放会失效）。

需要「普通权限启动、拖放正常」的变体（适合按用户安装、输出到文档/桌面）：

```bash
SPARROW_AS_INVOKER=1 cargo build --release
```

该变体写受保护目录时仍会失败，但程序会把输出自动改存到
`文档\SparrowPDF` 并在结果卡片提示（也可对目标目录做一次性授权：
`icacls "目标目录" /grant "%USERNAME%":(OI)(CI)M`，之后普通权限可直接写入）。

macOS 无此机制：权限由 POSIX/ACL 与 TCC 决定，`.app` 正常以当前用户身份运行。

## UI 语言与中文字体

界面为简体中文。为遵守体积红线，程序**不内嵌 CJK 字体**：启动时会按顺序探测系统字体（微软雅黑 / 等线 / 黑体 / 宋体 / PingFang / Noto Sans CJK / DejaVu Sans 等），命中后以运行时回退字体注入 egui。

## 光栅化的能力边界

「PDF 转图片」使用内置的软件光栅化器（`src/engine/render.rs` + `raster.rs` + `ttf.rs`），在进程内完成渲染，不依赖外部渲染器。它覆盖常见算符与对象：

- 路径：`m l c v y h re` + 填充（非零环绕 / 奇偶）/ 描边 / `B b` 系列；
- 色彩：灰度、RGB、CMYK，`gs` 中的 ca/CA 透明度；
- 图像：DCTDecode（JPEG）、FlateDecode、LZWDecode、CCITTFaxDecode（G3 一维 / 二维、G4）、JBIG2，
  以及无滤镜原样采样；支持 PNG / TIFF 预测器（`/Predictor` 2、10–15）、1/2/4/8/16 位采样与
  `/ImageMask` 模板蒙版；Form XObject 递归；
- 文本：`BT/ET` 块、Tf/Tm/Td/TD/T\*/TL/Tc/Tw/Tz/Tj/'/"/TJ；按 `/ToUnicode` 取字，其次
  `/Encoding /Differences`，最后按 WinAnsi 解释；**优先使用文档内嵌的 TrueType 程序**
  （`/FontFile2` 与 `/CIDToGIDMap`）绘制原始字形，未内嵌时以系统字体替换并逐字回退；
- 页面：MediaBox 与 /Rotate 沿页树 Parent 链继承。

**未覆盖的算符会被跳过**（如 shading pattern、Type3 字体、复杂 blend mode）：该页其余内容照常绘制，
缺少的部分在应用内预览与「PDF 转图片」产物里同样缺失，因此复杂文档的导出结果可能不完整。页面本身
无法解析（加密、损坏、无页面）时会显式报错，不产出空白文件。内嵌字体按原程序绘制；只有未内嵌的
字体才用系统字体替代，此时字形与字宽可能与专业渲染器存在差异。

## 依赖说明

| 依赖                              | 用途                     |
| --------------------------------- | ------------------------ |
| eframe / egui 0.37                | GUI（glow 渲染后端）     |
| egui_dnd 0.18                     | 列表拖拽排序             |
| rfd 0.17                          | 原生文件对话框           |
| lopdf 0.39                        | PDF 对象模型读写         |
| medpdf 0.9.2 / medpdf-image 0.2.2 | 页面操作 / JPEG 直通内嵌 |
| kamadak-exif 0.6                  | EXIF 方向读取            |
| image 0.25                        | PNG/JPEG 编解码          |
| weezl 0.2                         | LZW 解码（`/LZWDecode`） |
| hayro-ccitt 0.4                   | CCITT G3/G4 传真解码     |
| hayro-jbig2 0.3                   | JBIG2 解码               |
| flate2 1                          | zlib 解压（`/FlateDecode`） |
| allsorts 0.16                     | 系统 TrueType 字体解析   |

## License

本项目使用 [MIT](./LICENSE) 许可证授权。
