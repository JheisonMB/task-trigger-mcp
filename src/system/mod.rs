//! System monitoring with host-aware fallbacks.
//!
//! `sysinfo` provides process/runtime metrics, but under WSL it only sees the Linux
//! guest. For hardware-facing values like installed RAM and GPU, query the Windows
//! host when possible and fall back to platform-local commands elsewhere.

use serde::Deserialize;
use std::process::Command;
use sysinfo::{Components, System};

/// System information and metrics
#[derive(Debug, Default)]
pub struct SystemInfo {
    pub cpu_usage: f32,
    pub cpu_temperature: Option<f32>,
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
    cpu_usage: Option<f32>,
    cpu_temperature: Option<f32>,
    memory_used: Option<u64>,
    memory_total: Option<u64>,
    gpu_info: Option<GpuInfo>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WindowsHostSnapshot {
    cpu_temperature_c: Option<f32>,
    cpu_usage_percent: Option<f32>,
    installed_memory_bytes: Option<u64>,
    visible_memory_bytes: Option<u64>,
    free_memory_bytes: Option<u64>,
    gpu_usage_percent: Option<f32>,
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
        // Lightweight refresh: only CPU and memory, not all processes
        let mut system = System::new();
        system.refresh_cpu_usage();
        system.refresh_memory();

        self.cpu_usage = system.global_cpu_usage();
        self.cpu_temperature = None;
        self.memory_used = system.used_memory();
        self.memory_total = system.total_memory();
        self.system_uptime = System::uptime();
        self.process_count = system.processes().len();
        self.gpu_info = None;

        let component_metrics = self.read_component_metrics();
        self.cpu_temperature = component_metrics.cpu_temperature;
        if self.gpu_info.is_none() {
            self.gpu_info = component_metrics.gpu_info;
        }

        let host_metrics = self.try_get_host_metrics();
        if let Some(cpu_usage) = host_metrics.cpu_usage {
            self.cpu_usage = cpu_usage;
        }
        if let Some(cpu_temperature) = host_metrics.cpu_temperature {
            self.cpu_temperature = Some(cpu_temperature);
        }
        if let Some(memory_used) = host_metrics.memory_used {
            self.memory_used = memory_used;
        }
        if let Some(memory_total) = host_metrics.memory_total {
            self.memory_total = memory_total;
        }
        if let Some(gpu) = host_metrics.gpu_info {
            self.gpu_info = Some(gpu);
        }
    }

    fn read_component_metrics(&self) -> HostMetrics {
        let components = Components::new_with_refreshed_list();
        let mut cpu_temperature = None;
        let mut gpu_temperature = None;

        for component in &components {
            let Some(temperature) = normalize_temperature(component.temperature()) else {
                continue;
            };
            let label = component.label().to_ascii_lowercase();
            if cpu_temperature.is_none() && is_cpu_temperature_label(&label) {
                cpu_temperature = Some(temperature);
            }
            if gpu_temperature.is_none() && is_gpu_temperature_label(&label) {
                gpu_temperature = Some(temperature);
            }
        }

        HostMetrics {
            cpu_usage: None,
            cpu_temperature,
            memory_used: None,
            memory_total: None,
            gpu_info: gpu_temperature.map(|temperature| GpuInfo {
                temperature: Some(temperature),
                ..GpuInfo::default()
            }),
        }
    }

    fn try_get_host_metrics(&self) -> HostMetrics {
        match detect_host_platform() {
            HostPlatform::Wsl | HostPlatform::Windows => self.get_windows_host_metrics(),
            HostPlatform::Linux => HostMetrics {
                cpu_usage: None,
                cpu_temperature: None,
                memory_used: None,
                memory_total: None,
                gpu_info: self.get_linux_gpu_info().or_else(try_get_nvidia_gpu_info),
            },
            HostPlatform::MacOs => HostMetrics {
                cpu_usage: None,
                cpu_temperature: None,
                memory_used: None,
                memory_total: None,
                gpu_info: self.get_macos_gpu_info().or_else(try_get_nvidia_gpu_info),
            },
        }
    }

    fn get_windows_host_metrics(&self) -> HostMetrics {
        let snapshot = self.get_windows_host_snapshot();
        let mut gpu_info = try_get_nvidia_gpu_info().or_else(|| self.get_windows_gpu_info());
        if let Some(usage) = snapshot.gpu_usage_percent {
            if let Some(gpu) = gpu_info.as_mut() {
                gpu.usage = Some(usage);
            }
        }

        let memory_total = snapshot
            .installed_memory_bytes
            .or(snapshot.visible_memory_bytes)
            .filter(|total| *total > 0);

        let memory_used = match (snapshot.visible_memory_bytes, snapshot.free_memory_bytes) {
            (Some(visible), Some(free)) if visible >= free => Some(visible - free),
            _ => None,
        }
        .map(|used| match memory_total {
            Some(total) => used.min(total),
            None => used,
        });

        HostMetrics {
            cpu_usage: snapshot.cpu_usage_percent,
            cpu_temperature: snapshot.cpu_temperature_c,
            memory_used,
            memory_total,
            gpu_info,
        }
    }

    fn get_windows_host_snapshot(&self) -> WindowsHostSnapshot {
        let script = concat!(
            "$os = Get-CimInstance Win32_OperatingSystem; ",
            "$dimms = Get-CimInstance Win32_PhysicalMemory | Measure-Object -Property Capacity -Sum; ",
            "$cpuCounter = (Get-Counter '\\Processor(_Total)\\% Processor Time' -ErrorAction SilentlyContinue).CounterSamples | Select-Object -First 1 -ExpandProperty CookedValue; ",
            "$thermal = Get-CimInstance -Namespace root/wmi -Class MSAcpi_ThermalZoneTemperature -ErrorAction SilentlyContinue; ",
            "$cpuTemp = $null; ",
            "if ($thermal) { ",
            "  $temps = @($thermal | Where-Object { $_.CurrentTemperature -gt 0 } | ForEach-Object { ($_.CurrentTemperature / 10) - 273.15 }); ",
            "  if ($temps.Count -gt 0) { $cpuTemp = [math]::Round((($temps | Measure-Object -Average).Average), 1) } ",
            "} ",
            "$gpuUsage = $null; ",
            "try { ",
            "  $gpuCounters = (Get-Counter '\\GPU Engine(*)\\Utilization Percentage' -ErrorAction Stop).CounterSamples; ",
            "  $gpu3d = @($gpuCounters | Where-Object { $_.InstanceName -match 'engtype_3D' }); ",
            "  $samples = if ($gpu3d.Count -gt 0) { $gpu3d } else { $gpuCounters }; ",
            "  if ($samples.Count -gt 0) { ",
            "    $sum = ($samples | Measure-Object -Property CookedValue -Sum).Sum; ",
            "    if ($null -ne $sum) { $gpuUsage = [math]::Min([math]::Round([double]$sum, 1), 100) } ",
            "  } ",
            "} catch {} ",
            "[pscustomobject]@{",
            "CpuTemperatureC = $cpuTemp; ",
            "CpuUsagePercent = if ($null -ne $cpuCounter) { [math]::Round([double]$cpuCounter, 1) } else { $null }; ",
            "InstalledMemoryBytes = [uint64]($dimms.Sum); ",
            "VisibleMemoryBytes = [uint64]($os.TotalVisibleMemorySize * 1KB); ",
            "FreeMemoryBytes = [uint64]($os.FreePhysicalMemory * 1KB); ",
            "GpuUsagePercent = $gpuUsage",
            "} | ConvertTo-Json -Compress"
        );

        run_powershell_json::<WindowsHostSnapshot>(script).unwrap_or_default()
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

    pub fn cpu_temperature_celsius(&self) -> Option<f32> {
        self.cpu_temperature
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

fn try_get_nvidia_gpu_info() -> Option<GpuInfo> {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,utilization.gpu,temperature.gpu,memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let line = String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .find(|line| !line.trim().is_empty())?
        .to_string();
    let parts: Vec<&str> = line.split(',').map(|part| part.trim()).collect();
    if parts.is_empty() {
        return None;
    }

    Some(GpuInfo {
        name: parts.first()?.to_string(),
        vendor: "NVIDIA".to_string(),
        usage: parse_optional_f32(parts.get(1).copied()),
        temperature: parse_optional_f32(parts.get(2).copied()),
        vram_used: parse_optional_u64(parts.get(3).copied()),
        vram_total: parse_optional_u64(parts.get(4).copied()),
    })
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

fn parse_optional_f32(value: Option<&str>) -> Option<f32> {
    value
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<f32>().ok())
}

fn parse_optional_u64(value: Option<&str>) -> Option<u64> {
    value
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<u64>().ok())
}

fn normalize_temperature(value: Option<f32>) -> Option<f32> {
    value.filter(|temperature| temperature.is_finite() && *temperature > 0.0)
}

fn is_cpu_temperature_label(label: &str) -> bool {
    label.contains("cpu")
        || label.contains("package")
        || label.contains("tctl")
        || label.contains("tdie")
        || label.contains("coretemp")
}

fn is_gpu_temperature_label(label: &str) -> bool {
    label.contains("gpu") || label.contains("graphics") || label.contains("junction")
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

        assert!(info.memory_total >= info.memory_used);
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
