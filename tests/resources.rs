use std::path::Path;
use wifi_recovery_api::resources::Sampler;

#[test]
fn unavailable_resources_remain_unknown() {
    let directory = tempfile::tempdir().unwrap();
    let sample = Sampler::default().sample(directory.path(), directory.path(), None);
    assert_eq!(sample.cpu_percent, None);
    assert_eq!(sample.memory_total_bytes, None);
    assert_eq!(sample.memory_used_bytes, None);
    assert!(sample.gpus.is_empty());
    assert_eq!(sample.selected_gpu_pci, None);
}

fn write_cpu(root: &Path, counters: &str) {
    std::fs::write(root.join("stat"), counters).unwrap();
    std::fs::write(
        root.join("meminfo"),
        "MemTotal: 1000 kB\nMemAvailable: 600 kB\n",
    )
    .unwrap();
}

#[test]
fn server_cpu_deltas_do_not_include_guest_time_twice() {
    let directory = tempfile::tempdir().unwrap();
    let mut sampler = Sampler::default();
    write_cpu(directory.path(), "cpu 10 0 10 80 0 0 0 0 200 100\n");
    assert_eq!(
        sampler
            .sample(directory.path(), directory.path(), None)
            .cpu_percent,
        None
    );
    write_cpu(directory.path(), "cpu 35 0 35 130 0 0 0 0 300 200\n");
    let value = sampler.sample(directory.path(), directory.path(), None);
    assert_eq!(value.cpu_percent, Some(50.0));
    assert_eq!(value.memory_used_bytes, Some(400 * 1024));
    write_cpu(directory.path(), "cpu 1 0 1 5 0 0 0 0\n");
    assert_eq!(
        sampler
            .sample(directory.path(), directory.path(), None)
            .cpu_percent,
        None
    );
}

#[cfg(unix)]
#[test]
fn amd_devices_are_deduplicated_and_ambiguous_mapping_is_unknown() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let drm = root.join("drm");
    std::fs::create_dir(&drm).unwrap();
    for (name, pci) in [
        ("card0", "0000:01:00.0"),
        ("renderD128", "0000:01:00.0"),
        ("card1", "0000:02:00.0"),
    ] {
        let device = root.join(pci);
        std::fs::create_dir_all(&device).unwrap();
        for (file, text) in [
            ("vendor", "0x1002"),
            ("gpu_busy_percent", "75"),
            ("mem_info_vram_total", "1000"),
            ("mem_info_vram_used", "400"),
        ] {
            std::fs::write(device.join(file), text).unwrap();
        }
        std::fs::create_dir(drm.join(name)).unwrap();
        std::os::unix::fs::symlink(device, drm.join(name).join("device")).unwrap();
    }
    let mut sampler = Sampler::default();
    let sample = sampler.sample(root, &drm, None);
    assert_eq!(sample.gpus.len(), 2);
    assert_eq!(sample.selected_gpu_pci, None);
    assert_eq!(sample.gpus[0].utilization_percent, Some(75));
    assert_eq!(sample.gpus[0].memory_used_bytes, Some(400));
    assert_eq!(
        sampler
            .sample(root, &drm, Some("0000:02:00.0"))
            .selected_gpu_pci
            .as_deref(),
        Some("0000:02:00.0")
    );
}
