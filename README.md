# 轻雀 PDF（SparrowPDF）

> PDF 的事，本地办完。

面向日常 PDF 处理的桌面工具集：合并、拆分、旋转、删页、加水印、加密、转图片。全部在本机完成，
无网络请求、无遥测、无需账号。

![platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS-blue) ![rust](https://img.shields.io/badge/rust-1.95%2B-orange)

## 下载

| 你的系统 | 下载 | 说明 |
| --- | --- | --- |
| Windows 10 / 11（Intel、AMD） | `SparrowPDF-windows-x64.exe` | 免安装，解压即用 |
| Windows 10 / 11 on ARM | `SparrowPDF-windows-arm64.exe` | 原生 ARM64 构建 |
| macOS 11+（Intel 与 Apple Silicon） | `SparrowPDF-macos-universal.dmg` | 通用二进制，双架构共用 |

ARM 版适用于骁龙 8cx / X 系列等 Windows on ARM 设备，系统要求 Windows 10 及以上。

Windows 版默认请求管理员权限，原因见「Windows 的提权」。

macOS 包为 ad-hoc 签名、未经公证，首次打开可能提示「无法验证开发者」或「已损坏」。确认来源后，
在终端执行一次解除隔离：

```bash
xattr -dr com.apple.quarantine /Applications/SparrowPDF.app
```

## 能做什么

| 分组 | 工具 | 说明 |
| --- | --- | --- |
| 创建 | 图片合并 PDF | 选择多张 JPG / PNG 按顺序合成单个 PDF；竖拍照片自动校正方向 |
| 创建 | 批量转换 | 指定多个文件夹，各生成一个 PDF；文件名按数值排序 |
| 合并 | PDF 合并 | 拖拽调整顺序后合成单个 PDF |
| 拆分 | PDF 拆分 | 按页码范围、每 N 页、或一页一个文件 |
| 页面 | 页面旋转 | 每次旋转 90°，可连续操作，按累计角度生效 |
| 页面 | 删除页面 | 指定页码删除 |
| 页面 | 页面重排 | 拖拽排序，实时预览输出页序 |
| 页面 | 提取页面 | 选中的页单独存为新文件 |
| 水印 | 文本水印 | 平铺或居中，可调字号、透明度、角度，使用系统中文字体 |
| 水印 | 图片水印 | 支持缩放、透明度与叠加位置 |
| 安全 | 加密 / 解密 | AES-256，可分别限制打印 / 复制 / 修改；亦可移除密码 |
| 导出 | PDF 转图片 | 每页导出 PNG / JPEG，36–600 DPI（默认 300） |

## 使用要点

**同名文件不覆盖。** 程序先规划本次要写的全部文件，检测到重名时暂停并询问：覆盖、另存为新文件
（追加 `-2`、`-3`）或取消。批量任务一次性列出全部冲突，单次作答即可，可勾选「本次后续均执行
此操作」。

**外观跟随系统。** 侧边栏底部可切换浅色 / 深色，选择会持久化，下次启动沿用。

**版本号见「关于」。** 从侧边栏底部进入，显示完整五位版本（如 `V0.5.1.20261106`）。

**输出位置优先采用用户指定路径。** 未指定时写入源文件同目录；目录不可写（如 `Program Files`）
时自动改存至 `文档\SparrowPDF`，并在结果卡片中标注实际路径。

## Windows 的提权

默认版本启动时请求 UAC 提权，以便直接写入 `Program Files` 等受保护目录。代价是系统禁止向提权
窗口拖放文件；需要保留拖放时使用免提权变体：

```bash
SPARROW_AS_INVOKER=1 cargo build --release
```

该变体写受保护目录会失败，同样自动改存至 `文档\SparrowPDF`。

macOS 无此机制：权限由 POSIX/ACL 与 TCC 决定，`.app` 以当前用户身份运行。

## 从源码构建

```bash
cargo build --release
```

最低 Rust 1.95（edition 2021），产物为单个可执行文件。

## 已知限制

- 「PDF 转图片」使用内置渲染器，覆盖常见文档；shading、Type3 字体、复杂混合模式等暂不支持，
  遇到这类内容会跳过对应部分，页面其余内容照常输出；排版复杂的文档建议对照专业阅读器复核。
- 未内嵌字体的 PDF 以系统字体替代，字形与字宽可能与专业渲染器存在差异。
- 加密只支持 AES-256；不支持数字签名与表单填写。
- 文件损坏或密码错误时直接报错，不产出空白文件。

渲染器支持范围、实现细节与已知边界见 [docs/](./docs/)；协作者规范见 [AGENTS.md](./AGENTS.md)。

## License

本项目使用 [MIT](./LICENSE) 许可证授权。
