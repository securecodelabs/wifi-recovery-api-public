use crate::model::now_ms;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Resources {
    pub sampled_at_ms: u64,
    pub cpu_percent: Option<f64>,
    pub memory_total_bytes: Option<u64>,
    pub memory_available_bytes: Option<u64>,
    pub memory_used_bytes: Option<u64>,
    pub gpus: Vec<Gpu>,
    pub selected_gpu_pci: Option<String>,
    pub gpu_mapping: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Gpu {
    pub pci: String,
    pub utilization_percent: Option<u64>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
}

#[derive(Default)]
pub struct Sampler {
    previous: Option<(u64, u64)>,
}

impl Sampler {
    pub fn sample(
        &mut self,
        proc_root: &Path,
        drm_root: &Path,
        selected: Option<&str>,
    ) -> Resources {
        let counters = read(proc_root.join("stat")).and_then(|text| cpu_counters(&text));
        let cpu_percent = self
            .previous
            .zip(counters)
            .and_then(|(old, new)| cpu_percent(old, new));
        self.previous = counters;
        let memory = read(proc_root.join("meminfo"))
            .map(|text| memory(&text))
            .unwrap_or_default();
        let gpus = gpus(drm_root);
        let chosen = selected
            .map(str::to_owned)
            .or_else(|| (gpus.len() == 1).then(|| gpus[0].pci.clone()));
        Resources {
            sampled_at_ms: now_ms(),
            cpu_percent,
            memory_total_bytes: memory.0,
            memory_available_bytes: memory.1,
            memory_used_bytes: memory.0.zip(memory.1).and_then(|(t, a)| t.checked_sub(a)),
            gpus,
            selected_gpu_pci: chosen,
            gpu_mapping: if selected.is_some() {
                "configured_pci"
            } else {
                "single_amd_device_or_unknown"
            }
            .into(),
        }
    }
}

fn read(path: PathBuf) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .filter(|text| text.len() <= 1024 * 1024)
}

fn cpu_counters(text: &str) -> Option<(u64, u64)> {
    let mut fields = text.lines().next()?.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    let counters: Vec<u64> = fields
        .take(8)
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    if counters.len() < 4 {
        return None;
    }
    let total = counters
        .iter()
        .try_fold(0_u64, |sum, value| sum.checked_add(*value))?;
    let idle = counters[3].checked_add(*counters.get(4).unwrap_or(&0))?;
    Some((total, idle))
}

fn cpu_percent(old: (u64, u64), new: (u64, u64)) -> Option<f64> {
    let total = new.0.checked_sub(old.0).filter(|delta| *delta > 0)?;
    let idle = new.1.checked_sub(old.1)?;
    let busy = total.checked_sub(idle)?;
    Some(busy as f64 / total as f64 * 100.0)
}

fn memory(text: &str) -> (Option<u64>, Option<u64>) {
    let fields: BTreeMap<_, _> = text
        .lines()
        .filter_map(|line| line.split_once(':'))
        .collect();
    let bytes = |key| {
        fields.get(key).and_then(|text| {
            let mut parts = text.split_whitespace();
            let value: u64 = parts.next()?.parse().ok()?;
            (parts.next()? == "kB")
                .then(|| value.checked_mul(1024))
                .flatten()
        })
    };
    let total = bytes("MemTotal").filter(|value| *value > 0);
    let available =
        bytes("MemAvailable").filter(|value| total.is_some_and(|total| *value <= total));
    (total, available)
}

fn gpus(root: &Path) -> Vec<Gpu> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let devices: BTreeMap<_, _> = entries
        .flatten()
        .filter_map(|entry| {
            let device = entry.path().join("device").canonicalize().ok()?;
            if read(device.join("vendor"))?.trim() != "0x1002" {
                return None;
            }
            Some((device.file_name()?.to_str()?.to_owned(), device))
        })
        .collect();
    devices
        .into_iter()
        .map(|(pci, path)| {
            let value =
                |name| read(path.join(name)).and_then(|text| text.trim().parse::<u64>().ok());
            let total = value("mem_info_vram_total").filter(|n| *n > 0);
            Gpu {
                pci,
                utilization_percent: value("gpu_busy_percent").filter(|n| *n <= 100),
                memory_used_bytes: value("mem_info_vram_used")
                    .filter(|used| total.is_some_and(|t| *used <= t)),
                memory_total_bytes: total,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_delta_and_memory_are_bounded() {
        assert_eq!(
            cpu_counters("cpu 10 0 10 80 0 0 0 0 99 99\n"),
            Some((100, 80))
        );
        assert_eq!(cpu_percent((100, 80), (200, 130)), Some(50.0));
        assert_eq!(cpu_percent((100, 80), (50, 10)), None);
        assert_eq!(
            memory("MemTotal: 100 kB\nMemAvailable: 40 kB\n"),
            (Some(102400), Some(40960))
        );
        assert_eq!(memory("MemTotal: 100 kB\nMemAvailable: 101 kB\n").1, None);
    }
}
