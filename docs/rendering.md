# 内置光栅器

`engine/render.rs` 把 PDF 页面画成像素，供「PDF 转图片」与缩略图预览共用。不依赖外部
渲染器（不调用 mupdf / poppler / 系统 Print 管线），因此包体小、无外部进程，但能力边界
由自己实现的部分决定——这份文档写清边界在哪。

## 1. 流程

```
页面 → 解析内容流 → 解释算子 → raster 绘制 → 像素缓冲 → PNG / JPEG 编码
```

- 按 DPI 缩放（默认 300，范围 36–600，界面附耗时提示）。
- `render_page_thumbnails` 按页批量渲染缩略图；UI 侧每帧限量渲染，避免大文档卡顿。
- `render_page` 返回 `Result`：一条坏图或坏字体不该让整页失败。

## 2. 算子覆盖

- **路径**：`m l c v y h re`，非零环绕与奇偶两种填充规则、描边、`B b` 系列（填充并描边）。
- **色彩**：灰度 / RGB / CMYK；`gs` 里的 ca / CA 透明度。
- **页面**：MediaBox 与 `/Rotate` 沿页树 Parent 链继承。
- **未覆盖的算子**只跳过该算子，其余内容照常绘制。shading pattern、Type3 字体、
  复杂 blend mode 属于这一类，预览与导出会同时缺这部分，**不是** bug 回归。

填充遵循 PDF 语义：子路径隐式闭合（闭合边不能丢），水平方向按像素累计覆盖率后只合成
一次——早先逐子样本合成会让实心区只有 ~68% 强度。

## 3. 文本取字

字节是编码，不是字符。取字顺序：

1. `/ToUnicode` 反查（CID 字体按两字节码）；
2. `/Encoding /Differences`（`uniXXXX`、`uXXXX`、单字符名与常用标点名）；
3. 简单字体按 WinAnsi 解释（含 0x80–0x9F 专用区）；
4. 复合字体既无映射又无内嵌程序时**不画**，而不是画成方框。

## 4. 字形来源

- 优先用文档内嵌的 TrueType 程序（`/FontFile2`，经 `/FontDescriptor` 取得）。CID 字体按
  `/CIDToGIDMap` 把 CID 换成 glyph id，缺省按 `/Identity`。这样子集字体的任意编码都能画对。
- 文件未内嵌字体时，按 `/BaseFont` 选系统黑体 / 宋体 / 等宽替换，单字缺字形时逐字回退。
- 只为取轮廓而解析字体程序，不执行其中的指令；也不把内嵌程序塞进系统字体全局缓存——
  否则不同文档的同名字体会互相污染。

## 5. 图像解码

支持 `DCTDecode`、`FlateDecode`、`LZWDecode`（`/EarlyChange` 两种码宽规则）、
`CCITTFaxDecode`（K<0 为 G4，K=0 为 G3 一维，K>0 为 G3 二维）与 JBIG2（含
`/JBIG2Globals` 共享全局段）；支持 `/Predictor` 2（TIFF）与 10–15（PNG）、1/2/4/8/16 位
采样，`/ImageMask` 按当前填充色绘制。

三个容易踩的点：

- 无 `/Filter` 的流是原样采样，不能用 `decompressed_content`（lopdf 会因缺少 `/Filter`
  报错），要先判断再取 `content`。
- CCITT 的 `/BlackIs1` 描述采样打包方式，不影响画面；适配器一律输出「0 = 深色」的亮度栅格。
- 解码失败只跳过该图像，不得按原样当像素画（会得到噪点），也不得 panic。

## 6. 矩阵与表单

- **矩阵合成**：`raster::Matrix::mul` 必须与 `apply` 同一约定（列向量，
  `x' = a·x + c·y + e`）。两者不一致会让所有 `cm` / `/Rotate` / Form `/Matrix` 叠加后的
  几何整体错位（曾表现为整页空白）。改变换代码后跑 `matrix_mul_*` 单元测试。
- **表单资源**：Form XObject 自带 `/Resources` 时必须用该字典解析（`resources_from`）。
  沿用页面资源会让表单里的文字与图片用错字体表，表现为整段文字消失。

## 7. 缩略图与重排预览

`render_page_thumbnails` 负责批量出图；`preview::show_page_order` 按
`tools::reorder::ordered_pages` 的同一映射渲染输出页序，标签为输出页码。源页缩略图按页号
缓存，拖动只重排、不重渲染——预览与产物的页码序列由这条共享映射保证一致。

## 8. 已知缺口

- shading pattern（渐变）、Type3 字体、复杂 blend mode、透明度组（`/Group`）不绘制。
- 未内嵌字体的文档用系统字体替代，字形与字宽可能与专业渲染器有差异。
- 只做页面内容渲染：注释、表单域外观、可选内容组（OCG）的显隐状态不参与。
- 加密文档需先提供密码，密码错误不尝试空密码或其它回退。
