//! 版本号的唯一来源。
//!
//! 项目版本为五位：`V主版本.副版本.修订版本.日期`（如 `V0.5.0.20261104`）。
//! Cargo 只接受 SemVer 的 `主.副.修订` 三段，因此 `Cargo.toml` 保存前三段，
//! 日期位在这里维护，展示版本由两者拼接——任何界面或文档都不得另行拼字符串。
//!
//! 各位置的调整权（对齐《协作与交付规范》第 8 节）：
//! - 主版本：功能模块有较大变动（新增模块、整体架构变化），由项目经理决定；
//! - 副版本：功能有增加或变化（权限控制、自定义视图等），由项目经理决定；
//! - 修订版本：Bug 修复与小改动，可高频发布，由项目经理决定；
//! - 日期位：改动当天的日期，每天改动都要更新，由开发人员决定，且必须与发布
//!   tag 的日期段一致（发布流水线会校验，不一致直接失败）。

/// 核心版本（`主.副.修订`），编译期取自 `Cargo.toml`。
pub const CORE: &str = env!("CARGO_PKG_VERSION");

/// 日期位（`YYYYMMDD`），记录本次改动的日期。
///
/// 发布当天由开发人员更新为当天日期；与 tag 的日期段不一致时 CI 会拦下。
pub const DATE: &str = "20261104";

/// 展示用完整版本，如 `V0.5.0.20261104`。
///
/// 只拼接一次并缓存：侧边栏与关于弹窗每帧都要读它，没必要反复分配。
pub fn display() -> &'static str {
    static DISPLAY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DISPLAY.get_or_init(|| format!("V{CORE}.{DATE}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_has_five_positions() {
        let value = display();
        let rest = value
            .strip_prefix('V')
            .unwrap_or_else(|| panic!("版本前缀必须是 V：{value}"));
        let parts: Vec<&str> = rest.split('.').collect();
        assert_eq!(parts.len(), 4, "除前缀外应有主/副/修订/日期四段：{value}");
        assert_eq!(
            parts[..3].join("."),
            CORE,
            "前三段必须与 Cargo 版本一致：{value}"
        );
        assert_eq!(parts[3], DATE, "日期段必须与 DATE 常量一致：{value}");
    }

    #[test]
    fn date_position_is_a_real_date() {
        assert_eq!(DATE.len(), 8, "日期位必须是 8 位 YYYYMMDD：{DATE}");
        assert!(
            DATE.bytes().all(|b| b.is_ascii_digit()),
            "日期位只能包含数字：{DATE}"
        );
        let year: u32 = DATE[0..4].parse().expect("年份段必须是数字");
        let month: u32 = DATE[4..6].parse().expect("月份段必须是数字");
        let day: u32 = DATE[6..8].parse().expect("日段必须是数字");
        assert!((2000..=2999).contains(&year), "年份超出合理范围：{DATE}");
        assert!((1..=12).contains(&month), "月份必须是 1-12：{DATE}");
        assert!((1..=31).contains(&day), "日必须是 1-31：{DATE}");
    }
}
