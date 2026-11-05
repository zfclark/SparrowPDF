# 轻雀 PDF（SparrowPDF）

> PDF 的事，本地办完。

基于 Rust 的本地 PDF 工具箱：不联网、不上传、无遥测，处理全在本机完成。

![platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS-blue) ![rust](https://img.shields.io/badge/rust-1.95%2B-orange)

## 功能（12 项）

| 分组 | 工具         | 说明                                                            |
| ---- | ------------ | --------------------------------------------------------------- |
| 创建 | 图片合并 PDF | JPEG 走 DCTDecode 直通并纠正 EXIF 方向；PNG 以 FlateDecode 内嵌 |
| 创建 | 批量转换     | 每个文件夹一个 PDF，文件名数字感知排序；可两图一页              |
| 合并 | PDF 合并     | 拖拽排序后按序合并                                              |
| 拆分 | PDF 拆分     | 按页码范围 / 每 N 页 / 逐页                                     |
| 页面 | 页面旋转     | 90° 增量，多次操作按累计语义合并                                |
| 页面 | 删除页面     | 按页码范围删除                                                  |
| 页面 | 页面重排     | 拖拽排序，实时预览输出页序                                      |
| 页面 | 提取页面     | 抽取选中页为新文件                                              |
| 水印 | 文本水印     | 平铺或居中；系统 CJK 字体；透明度与旋转角                       |
| 水印 | 图片水印     | 缩放适配、透明度、覆盖层                                        |
| 安全 | 加密 / 解密  | AES-256，可组合打印 / 复制 / 修改权限                           |
| 导出 | PDF 转图片   | 内置软件光栅化，PNG / JPEG，36–600 DPI（默认 300）              |

## 界面

- 侧边栏底部可切换**浅色 / 深色**，即时生效；选择写入 `%LOCALAPPDATA%\sparrowpdf\ui.pref`，
  未手动选择过则跟随系统主题。
- 「关于」入口显示完整版本号、作者与项目地址。

## 保存与同名文件

同名文件不覆盖：先规划本次要写的全部文件名，命中即停下询问「覆盖 / 另存为新文件（追加
`-2`、`-3`）/ 取消」。批量任务一次汇总、一次作答，可勾选「本次后续均执行此操作」。

## 构建

```bash
cargo build --release
```

最低 Rust 1.95（edition 2021）。

### Windows：提权与免提权变体

默认 exe 内嵌 `requestedExecutionLevel = requireAdministrator`，可直接写入受保护目录
（如 `Program Files`）；代价是每次启动弹 UAC，且资源管理器拖放到提权窗口失效。

需要「普通权限启动、拖放正常」时：

```bash
SPARROW_AS_INVOKER=1 cargo build --release
```

该变体写受保护目录会失败，此时程序自动改存 `文档\SparrowPDF` 并在结果卡片提示；也可对目标
目录一次性授权：`icacls "目标目录" /grant "%USERNAME%":(OI)(CI)M`。

macOS 无此机制：权限由 POSIX/ACL 与 TCC 决定，`.app` 以当前用户身份运行。

### 发布产物

| 附件                             | 适用                                   |
| -------------------------------- | -------------------------------------- |
| `SparrowPDF-windows-x64.exe`     | Windows 10 / 11 x64                    |
| `SparrowPDF-windows-arm64.exe`   | Windows 11 on ARM                      |
| `SparrowPDF-macos-universal.dmg` | macOS 11+，Intel 与 Apple Silicon 通用 |

macOS 包为 ad-hoc 签名、未经公证，首次打开可能被 Gatekeeper 拦下；确认来源后执行
`xattr -dr com.apple.quarantine /Applications/SparrowPDF.app` 放行。

## 中文字体

界面为简体中文，但不内嵌 CJK 字体（体积红线）：启动时按序探测系统字体（微软雅黑 / 等线 /
黑体 / 宋体 / PingFang / Noto Sans CJK / DejaVu Sans 等），命中后注入 egui 作为回退字体。

## 光栅化边界

「PDF 转图片」使用内置软件光栅化器（`engine/render.rs` + `raster.rs` + `ttf.rs`），不依赖外部渲染器：

- 路径：`m l c v y h re`，非零环绕 / 奇偶填充、描边、`B b` 系列；
- 色彩：灰度 / RGB / CMYK，`gs` 的 ca/CA 透明度；
- 图像：DCTDecode、FlateDecode、LZWDecode、CCITTFaxDecode（G3 一维 / 二维、G4）、JBIG2 与
  无滤镜原样采样；PNG / TIFF 预测器（2、10–15）、1/2/4/8/16 位采样、`/ImageMask` 模板蒙版；
  Form XObject 递归；
- 文本：`BT/ET` 与 `Tf/Tm/Td/TD/T*/TL/Tc/Tw/Tz/Tj/'/"/TJ`；按 `/ToUnicode` 取字，其次
  `/Encoding /Differences`，最后按 WinAnsi；优先用文档内嵌的 TrueType 程序（`/FontFile2`、
  `/CIDToGIDMap`）绘制原字形，未内嵌时以系统字体替换并逐字回退；
- 页面：MediaBox 与 `/Rotate` 沿页树 Parent 链继承。

未覆盖的算符会被跳过（shading pattern、Type3 字体、复杂 blend mode 等），该页其余内容照常
绘制，缺失部分在预览与导出中同样缺失。页面本身无法解析（加密、损坏、无页面）时显式报错，
不产出空白文件；未内嵌字体用系统字体替代，字形与字宽可能与专业渲染器存在差异。

## 依赖

eframe / egui 0.37（glow）、egui_dnd 0.18、rfd 0.17、lopdf 0.39、medpdf 0.9.2、
kamadak-exif 0.6、image 0.25、allsorts 0.16、weezl 0.2、hayro-ccitt 0.4、hayro-jbig2 0.3、
flate2 1（rust_backend）。

## License

本项目使用 [MIT](./LICENSE) 许可证授权。
