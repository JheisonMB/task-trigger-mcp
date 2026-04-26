//! System monitoring with host-aware fallbacks.
//!
//! `sysinfo` provides process/runtime metrics, but under WSL it only sees the Linux
//! guest. For hardware-facing values like installed RAM and GPU, query the Windows
//! host when possible and fall back to platform-local commands elsewhere.

use serde::Deserialize;
use std::process::Command;
use sysinfo::System;

/// System information and metrics
#[derive(Debug, Default)]
pub struct SystemInfo {
    pub cpu_usage: f32,
    pub memory_used: u64,
    pub memory_total: u64,
    pub system_uptime: u64,
    pub process_count: usize,
    pub gpu_info: Option<GpuInfo>,
}

/// GPU information
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct GpuInfo {
    pub name: String,
    pub vendor: String,
    pub usage: Option<f32>,
    pub temperature: Option<f32>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostPlatform {
    Linux,
    MacOs,
    Windows,
    Wsl,
}

#[derive(Debug, Default)]
struct HostMetrics {
    memory_used: Option<u64>,
    memory_total: Option<u64>,
    gpu_info: Option<GpuInfo>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WindowsHostMemory {
    installed_memory_bytes: Option<u64>,
    visible_memory_bytes: Option<u64>,
    free_memory_bytes: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WindowsVideoController {
    name: Option<String>,
    adapter_compatibility: Option<String>,
    adapter_ram: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    fn into_vec(self) -> Vec<T> {
        match self {
            Self::One(item) => vec![item],
            Self::Many(items) => items,
        }
    }
}

impl SystemInfo {
    pub fn new() -> Self {
        let mut this = Self::default();
        this.update();
        this
    }

    pub fn update(&mut self) {
        let mut system = System::new_all();
        system.refresh_all();

        self.cpu_usage = system.global_cpu_usage();
        self.memory_used = system.used_memory();
        self.memory_total = system.total_memory();
        self.system_uptime = System::uptime();
        self.process_count = system.processes().len();
        self.gpu_info = None;

        let host_metrics = self.try_get_host_metrics();
        if let Some(memory_used) = host_metrics.memory_used {
            self.memory_used = memory_used;
        }
        if let Some(memory_total) = host_metrics.memory_total {
            self.memory_total = memory_total;
        }
        self.gpu_info = host_metrics.gpu_info;
    }

    fn try_get_host_metrics(&self) -> HostMetrics {
        match detect_host_platform() {
            HostPlatform::Wsl | HostPlatform::Windows => self.get_windows_host_metrics(),
            HostPlatform::Linux => HostMetrics {
                memory_used: None,
                memory_total: None,
                gpu_info: self.get_linux_gpu_info(),
            },
            HostPlatform::MacOs => HostMetrics {
                memory_used: None,
                memory_total: None,
                gpu_info: self.get_macos_gpu_info(),
            },
        }
    }

    fn get_windows_host_metrics(&self) -> HostMetrics {
        let memory = self.get_windows_host_memory();
        let gpu_info = self.get_windows_gpu_info();

        let memory_total = memory
            .installed_memory_bytes
            .or(memory.visible_memory_bytes)
            .filter(|total| *total > 0);

        let memory_used = match (memory.visible_memory_bytes, memory.free_memory_bytes) {
            (Some(visible), Some(free)) if visible >= free => Some(visible - free),
            _ => None,
        }
        .map(|used| match memory_total {
            Some(total) => used.min(total),
            None => used,
        });

        HostMetrics {
            memory_used,
            memory_total,
            gpu_info,
        }
    }

    fn get_windows_host_memory(&self) -> WindowsHostMemory {
        let script = concat!(
            "$os = Get-CimInstance Win32_OperatingSystem; ",
            "$dimms = Get-CimInstance Win32_PhysicalMemory | Measure-Object -Property Capacity -Sum; ",
            "[pscustomobject]@{",
            "InstalledMemoryBytes = [uint64]($dimms.Sum); ",
            "VisibleMemoryBytes = [uint64]($os.TotalVisibleMemorySize * 1KB); ",
            "FreeMemoryBytes = [uint64]($os.FreePhysicalMemory * 1KB)",
            "} | ConvertTo-Json -Compress"
        );

        run_powershell_json::<WindowsHostMemory>(script).unwrap_or_default()
    }

    fn get_windows_gpu_info(&self) -> Option<GpuInfo> {
        let script = concat!(
            "Get-CimInstance Win32_VideoController ",
            "| Where-Object { $_.Name -and $_.Name -notmatch 'Microsoft Basic|Remote Display|Hyper-V' } ",
            "| Select-Object Name,AdapterCompatibility,AdapterRAM ",
            "| ConvertTo-Json -Compress"
        );

        let controllers = run_powershell_json::<OneOrMany<WindowsVideoController>>(script)?
            .into_vec()
            .into_iter()
            .filter(|controller| {
                controller
                    .name
                    .as_ref()
                    .is_some_and(|name| !name.trim().is_empty())
            })
            .collect::<Vec<_>>();

        let controller = controllers.into_iter().next()?;
        let name = controller.name?.trim().to_string();
        let vendor = controller
            .adapter_compatibility
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| infer_gpu_vendor(&name));

        Some(GpuInfo {
            name,
            vendor,
            usage: None,
            temperature: None,
            vram_used: None,
            vram_total: controller.adapter_ram.map(bytes_to_megabytes),
        })
    }

    fn get_linux_gpu_info(&self) -> Option<GpuInfo> {
        let output = Command::new("lspci").output().ok()?;
        if !output.status.success() {
            return None;
        }

        let stdout = String::from_utf8(output.stdout).ok()?;
        stdout
            .lines()
            .find(|line| {
                line.contains(" VGA ")
                    || line.contains("3D controller")
                    || line.contains("Display controller")
            })
            .map(parse_lspci_gpu_line)
    }

    fn get_macos_gpu_info(&self) -> Option<GpuInfo> {
        let output = Command::new("system_profiler")
            .arg("SPDisplaysDataType")
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }

        let stdout = String::from_utf8(output.stdout).ok()?;
        stdout
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with("Chipset Model:"))
            .and_then(|line| line.split_once(':'))
            .map(|(_, value)| value.trim().to_string())
            .filter(|name| !name.is_empty())
            .map(|name| GpuInfo {
                vendor: infer_gpu_vendor(&name),
                name,
                usage: None,
                temperature: None,
                vram_used: None,
                vram_total: None,
            })
    }

    pub fn cpu_usage_percent(&self) -> f32 {
        self.cpu_usage
    }

    pub fn memory_used_gb(&self) -> f32 {
        bytes_to_gigabytes(self.memory_used)
    }

    pub fn memory_total_gb(&self) -> f32 {
        bytes_to_gigabytes(self.memory_total)
    }

    #[allow(dead_code)]
    pub fn gpu_vram_used_mb(&self) -> Option<u64> {
        self.gpu_info.as_ref()?.vram_used
    }

    #[allow(dead_code)]
    pub fn gpu_vram_total_mb(&self) -> Option<u64> {
        self.gpu_info.as_ref()?.vram_total
    }

    #[allow(dead_code)]
    pub fn gpu_vram_usage_percent(&self) -> Option<f32> {
        if let Some(gpu) = &self.gpu_info {
            if let (Some(used), Some(total)) = (gpu.vram_used, gpu.vram_total) {
                if total > 0 {
                    return Some((used as f32 / total as f32) * 100.0);
                }
            }
        }
        None
    }

    pub fn format_uptime(&self) -> String {
        let seconds = self.system_uptime;
        let minutes = seconds / 60;
        let hours = minutes / 60;
        let days = hours / 24;

        if days > 0 {
            format!("{}d {}h", days, hours % 24)
        } else if hours > 0 {
            format!("{}h {}m", hours, minutes % 60)
        } else if minutes > 0 {
            format!("{}m", minutes)
        } else {
            format!("{}s", seconds)
        }
    }
}

fn detect_host_platform() -> HostPlatform {
    if cfg!(target_os = "windows") {
        return HostPlatform::Windows;
    }
    if cfg!(target_os = "macos") {
        return HostPlatform::MacOs;
    }
    if let Ok(version) = std::fs::read_to_string("/proc/version") {
        if version.to_lowercase().contains("microsoft") {
            return HostPlatform::Wsl;
        }
    }
    HostPlatform::Linux
}

fn run_powershell_json<T>(script: &str) -> Option<T>
where
    T: for<'de> Deserialize<'de>,
{
    let output = Command::new("powershell.exe")
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg(script)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }

    serde_json::from_str(trimmed).ok()
}

fn parse_lspci_gpu_line(line: &str) -> GpuInfo {
    let name = line
        .split_once(": ")
        .map(|(_, value)| value.trim().to_string())
        .unwrap_or_else(|| line.trim().to_string());

    GpuInfo {
        vendor: infer_gpu_vendor(&name),
        name,
        usage: None,
        temperature: None,
        vram_used: None,
        vram_total: None,
    }
}

fn infer_gpu_vendor(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if lower.contains("nvidia") || lower.contains("geforce") || lower.contains("quadro") {
        "NVIDIA".to_string()
    } else if lower.contains("amd") || lower.contains("radeon") || lower.contains("ati") {
        "AMD".to_string()
    } else if lower.contains("intel") || lower.contains("arc") || lower.contains("uhd") {
        "Intel".to_string()
    } else if lower.contains("apple") {
        "Apple".to_string()
    } else {
        "GPU".to_string()
    }
}

fn bytes_to_gigabytes(bytes: u64) -> f32 {
    bytes as f32 / 1024.0 / 1024.0 / 1024.0
}

fn bytes_to_megabytes(bytes: u64) -> u64 {
    bytes / 1024 / 1024
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_info_creation() {
        let info = SystemInfo::new();
        assert!(info.cpu_usage >= 0.0 && info.cpu_usage <= 100.0);
        assert!(info.memory_total >= info.memory_used);
        assert!(info.system_uptime > 0);
    }

    #[test]
    fn test_memory_calculations() {
        let info = SystemInfo::new();
        let percent = if info.memory_total > 0 {
            (info.memory_used as f32 / info.memory_total as f32) * 100.0
        } else {
            0.0
        };
        assert!((0.0..=100.0).contains(&percent));

        let used_gb = info.memory_used_gb();
        let total_gb = info.memory_total_gb();
        assert!(used_gb >= 0.0);
        assert!(total_gb >= used_gb);
    }

    #[test]
    fn test_uptime_formatting() {
        let info = SystemInfo::new();
        let formatted = info.format_uptime();
        assert!(!formatted.is_empty());
    }

    #[test]
    fn test_parse_lspci_gpu_line_preserves_device_name() {
        let gpu = parse_lspci_gpu_line(
            "01:00.0 VGA compatible controller: NVIDIA Corporation AD106M [GeForce RTX 4070 Max-Q / Mobile]",
        );
        assert!(gpu.name.contains("GeForce RTX 4070"));
        assert_eq!(gpu.vendor, "NVIDIA");
    }

    #[test]
    fn test_infer_gpu_vendor() {
        assert_eq!(infer_gpu_vendor("Intel UHD Graphics"), "Intel");
        assert_eq!(infer_gpu_vendor("AMD Radeon RX 7800 XT"), "AMD");
        assert_eq!(infer_gpu_vendor("Apple M3"), "Apple");
    }
}
