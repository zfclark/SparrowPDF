<!--
标题沿用 Conventional Commits：<type>(<scope>): <中文摘要>（AGENTS.md 6.3）
目标分支：日常改动选 dev；发版 PR 由维护者发起，base 选 main、head 选 dev。
发版 PR 只需填「变更内容 / 验证方式 / 回滚方案」，其余可略。
-->

## 变更内容

<!-- 一句话说明这个 PR 解决什么问题；没有关联 issue 时写明触发场景 -->

## 关联需求

Closes #

## 影响面

- 涉及模块：
- 用户可见变化：无 / 有（说明）
- 兼容性：无 / 有（说明迁移方式）

## 验证方式

- [ ] `cargo clippy --release --all-targets -- -D warnings` 零警告
- [ ] `cargo test --release --lib --test integration` 全通过
- [ ] 动过依赖时 `cargo audit` 无新增漏洞（若有告警：修或写明理由）
- [ ] 涉及界面时已实际启动确认，无渲染异常与布局裁切（附截图）
- [ ] 用户可见变化已写入 `CHANGELOG.md` 的 `[Unreleased]`
- [ ] 新增或修改工具行为时补了集成测试，且断言产物结构而非「执行成功」

## 回滚方案

<!-- 撤销方式：回滚提交 / 关闭开关 / 恢复默认值 -->
