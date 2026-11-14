//! Read-only host information for the About dialog.
//!
//! Only what a bug report needs: the OS name/version and the CPU model. Each
//! probe runs once per session and is cached; a probe that fails returns
//! `None` so the dialog falls back to a shorter line instead of showing a
//! half-filled string.

use std::sync::OnceLock;

/// CPU model, e.g. `12th Gen Intel(R) Core(TM) i5-12400`.
pub fn cpu_model() -> Option<&'static str> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE.get_or_init(probe_cpu_model).as_deref()
}

/// Operating system with edition and build, e.g.
/// `Windows 11 专业版 23H2（22631）`.
pub fn os_version() -> Option<&'static str> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE.get_or_init(probe_os_version).as_deref()
}

/// Plain OS family, used when [`os_version`] cannot be read.
pub fn os_family() -> &'static str {
    match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        other => other,
    }
}

// ----------------------------------------------------------------- windows

#[cfg(target_os = "windows")]
fn probe_cpu_model() -> Option<String> {
    reg_string(
        r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
        "ProcessorNameString",
    )
}

#[cfg(target_os = "windows")]
fn probe_os_version() -> Option<String> {
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let product = reg_string(KEY, "ProductName")?;
    let build = reg_string(KEY, "CurrentBuildNumber").unwrap_or_default();
    let display = reg_string(KEY, "DisplayVersion").unwrap_or_default();

    // Windows 11 still reports "Windows 10" in ProductName; the build number
    // is the documented discriminator.
    let build_number: u32 = build.parse().unwrap_or(0);
    let product = if build_number >= 22000 {
        product.replace("Windows 10", "Windows 11")
    } else {
        product
    };
    let product = localize_edition(&product);

    Some(match (display.is_empty(), build.is_empty()) {
        (false, false) => format!("{product} {display}（{build}）"),
        (false, true) => format!("{product} {display}"),
        (true, false) => format!("{product}（{build}）"),
        (true, true) => product,
    })
}

/// Translate the English edition suffix of `ProductName` so the dialog stays
/// one language. Unknown editions are left untouched.
#[cfg(target_os = "windows")]
fn localize_edition(product: &str) -> String {
    // Longest first: "Pro for Workstations" must win over "Pro".
    const EDITIONS: &[(&str, &str)] = &[
        ("Pro for Workstations", "专业工作站版"),
        ("Enterprise LTSC", "企业版 LTSC"),
        ("Pro Education", "专业教育版"),
        ("Home Single Language", "家庭单语言版"),
        ("Enterprise", "企业版"),
        ("Education", "教育版"),
        ("Pro", "专业版"),
        ("Home", "家庭版"),
    ];
    for (english, chinese) in EDITIONS {
        if let Some(pos) = product.rfind(english) {
            return format!("{}{chinese}", &product[..pos]);
        }
    }
    product.to_string()
}

/// Read a `REG_SZ` value through `RegGetValueW`.
///
/// The registry is used instead of spawning `reg.exe`: no console window, no
/// encoding guesswork (the API returns UTF-16), and nothing to trip over on
/// machines with a restricted execution policy.
#[cfg(target_os = "windows")]
fn reg_string(subkey: &str, value: &str) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

    fn wide(text: &str) -> Vec<u16> {
        std::ffi::OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let subkey = wide(subkey);
    let value = wide(value);
    let mut buffer = [0u16; 512];
    let mut bytes = (buffer.len() * 2) as u32;

    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != 0 {
        return None;
    }

    // `bytes` counts a trailing NUL in UTF-16 units.
    let len = (bytes as usize / 2).saturating_sub(1).min(buffer.len());
    let text = String::from_utf16_lossy(&buffer[..len]);
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

// -------------------------------------------------------------------- macOS

#[cfg(target_os = "macos")]
fn probe_cpu_model() -> Option<String> {
    // Apple Silicon often reports an empty brand string, so fall back to the
    // hardware identifier (e.g. `MacBookPro18,3`) rather than showing nothing.
    // Absolute paths: a bundle launched from Finder gets a minimal PATH.
    command_line("/usr/sbin/sysctl", &["-n", "machdep.cpu.brand_string"])
        .or_else(|| command_line("/usr/sbin/sysctl", &["-n", "hw.model"]))
}

#[cfg(target_os = "macos")]
fn probe_os_version() -> Option<String> {
    let product = command_line("/usr/bin/sw_vers", &["-productName"])?;
    let version = command_line("/usr/bin/sw_vers", &["-productVersion"]).unwrap_or_default();
    let build = command_line("/usr/bin/sw_vers", &["-buildVersion"]).unwrap_or_default();
    Some(match (version.is_empty(), build.is_empty()) {
        (false, false) => format!("{product} {version}（{build}）"),
        (false, true) => format!("{product} {version}"),
        _ => product,
    })
}

#[cfg(target_os = "macos")]
fn command_line(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

// -------------------------------------------------------------------- Linux

#[cfg(all(unix, not(target_os = "macos")))]
fn probe_cpu_model() -> Option<String> {
    let text = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("model name"))
        .and_then(|rest| rest.split_once(':'))
        .map(|(_, name)| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn probe_os_version() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|name| name.trim_matches('"').to_string())
        .filter(|name| !name.is_empty())
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::localize_edition;

    #[test]
    fn edition_suffix_is_translated() {
        assert_eq!(localize_edition("Windows 10 Pro"), "Windows 10 专业版");
        assert_eq!(
            localize_edition("Windows 10 Pro for Workstations"),
            "Windows 10 专业工作站版"
        );
        assert_eq!(localize_edition("Windows 11 Home"), "Windows 11 家庭版");
        assert_eq!(
            localize_edition("Windows 10 Enterprise LTSC"),
            "Windows 10 企业版 LTSC"
        );
    }

    #[test]
    fn unknown_edition_is_left_alone() {
        assert_eq!(localize_edition("Windows Server 2022"), "Windows Server 2022");
    }
}
