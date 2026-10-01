//! Power / battery state (§31 Performance Mode: Battery Saver).

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
use crate::error::PlatformError;
use crate::error::PlatformResult;

/// Power state of the host.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PowerStatus {
    /// `Some(true)` on AC power, `Some(false)` on battery, `None` unknown.
    pub on_ac_power: Option<bool>,
    /// Battery charge 0..=100, `None` when there is no battery / unknown.
    pub battery_percent: Option<f32>,
    /// Whether a battery is present.
    pub has_battery: bool,
}

/// Power information.
pub trait Power: Send + Sync {
    /// Current status.
    fn status(&self) -> PlatformResult<PowerStatus>;
}

/// Native implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct NativePower;

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn windows_status() -> PlatformResult<PowerStatus> {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    // SAFETY: SYSTEM_POWER_STATUS is a plain-old-data struct for which an
    // all-zero bit pattern is valid; GetSystemPowerStatus only writes into the
    // pointed-to struct, which lives on our stack for the whole call.
    let mut s: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    // SAFETY: `&mut s` is a valid, aligned, writable pointer (see above).
    let ok = unsafe { GetSystemPowerStatus(&mut s) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    const NO_BATTERY: u8 = 128;
    const UNKNOWN: u8 = 255;
    let has_battery = s.BatteryFlag != NO_BATTERY && s.BatteryFlag != UNKNOWN;
    Ok(PowerStatus {
        on_ac_power: match s.ACLineStatus {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        },
        battery_percent: (has_battery && s.BatteryLifePercent != UNKNOWN).then_some(f32::from(s.BatteryLifePercent)),
        has_battery,
    })
}

#[cfg(target_os = "linux")]
fn linux_status() -> PlatformResult<PowerStatus> {
    let mut status = PowerStatus { on_ac_power: None, battery_percent: None, has_battery: false };
    let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") else {
        return Ok(status);
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let kind = std::fs::read_to_string(p.join("type")).unwrap_or_default();
        match kind.trim() {
            "Mains" => {
                let online = std::fs::read_to_string(p.join("online")).unwrap_or_default();
                status.on_ac_power = Some(online.trim() == "1");
            }
            "Battery" => {
                status.has_battery = true;
                if let Ok(c) = std::fs::read_to_string(p.join("capacity")) {
                    status.battery_percent = c.trim().parse::<f32>().ok();
                }
            }
            _ => {}
        }
    }
    Ok(status)
}

#[cfg(target_os = "macos")]
fn macos_status() -> PlatformResult<PowerStatus> {
    let out = crate::error::run_command("pmset", &["-g", "batt"])?;
    Ok(parse_pmset(&String::from_utf8_lossy(&out.stdout)))
}

/// Parses `pmset -g batt` output.
#[cfg(any(target_os = "macos", test))]
fn parse_pmset(text: &str) -> PowerStatus {
    let on_ac = if text.contains("'AC Power'") {
        Some(true)
    } else if text.contains("'Battery Power'") {
        Some(false)
    } else {
        None
    };
    let percent = text.split_whitespace().find_map(|w| w.trim_end_matches(';').strip_suffix('%')?.parse::<f32>().ok());
    PowerStatus { on_ac_power: on_ac, battery_percent: percent, has_battery: percent.is_some() }
}

impl Power for NativePower {
    fn status(&self) -> PlatformResult<PowerStatus> {
        #[cfg(target_os = "windows")]
        {
            windows_status()
        }
        #[cfg(target_os = "linux")]
        {
            linux_status()
        }
        #[cfg(target_os = "macos")]
        {
            macos_status()
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            Err(PlatformError::not_supported("power status", "unsupported OS"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_status_does_not_fail() {
        let s = NativePower.status().unwrap();
        if let Some(p) = s.battery_percent {
            assert!((0.0..=100.0).contains(&p));
        }
    }

    #[test]
    fn pmset_parsing() {
        let s = parse_pmset(
            "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t87%; discharging; 4:01 remaining",
        );
        assert_eq!(s.on_ac_power, Some(false));
        assert_eq!(s.battery_percent, Some(87.0));
        let s = parse_pmset("Now drawing from 'AC Power'");
        assert_eq!(s.on_ac_power, Some(true));
        assert!(!s.has_battery);
    }
}
