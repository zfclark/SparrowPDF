# 内置补丁依赖（vendor）

这里放两个第三方 crate 的副本，**功能源码零改动**，只改了一行依赖声明，用来解开
GHSA-6fj4-rcr3-r7rr 的版本死锁。

| 目录 | 上游 | 版本 | 许可证 |
| --- | --- | --- | --- |
| `medpdf/` | crates.io `medpdf` | 0.9.2 | MIT OR Apache-2.0 |
| `medpdf-image/` | crates.io `medpdf-image` | 0.2.2 | MIT OR Apache-2.0 |

## 为什么要 patch

- `lopdf` 0.39 及以下解析嵌套数组/字典时**没有递归上限**，约一万层的构造 PDF 就能让
  进程栈溢出并以 `SIGABRT` 退出（`catch_unwind` 拦不住），修复版是 0.42.0。
- `medpdf` 0.9.2 与 `medpdf-image` 0.2.2 都把依赖写成 `lopdf = "0.39.0"`（即 `^0.39`，
  不含 0.40+），上游至今没有支持 0.42 的版本，`cargo update` 满足不了。

## 与上游的差异（只有这些）

1. `Cargo.toml`：`[dependencies.lopdf] version = "0.39.0"` → `"0.42"`。
2. `Cargo.toml`：删掉 `[[test]]` 段（随包发布的 `tests/` 未引入，不参与本仓库构建）。
3. 删掉随包发布的 `Cargo.lock`（本仓库用自己的锁文件）。

`src/` 一行未改：两个 crate 在 `lopdf` 0.42 上原样编译通过，行为不变。

## 怎么撤掉

上游发布基于 `lopdf` 0.42 的版本后：

1. 删掉本目录；
2. 删掉根 `Cargo.toml` 里 `[patch.crates-io]` 的两行；
3. 把 `medpdf` / `medpdf-image` 的版本号提到新版本，`cargo update`。
