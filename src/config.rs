use clap::Parser;
use std::{net::SocketAddr, path::PathBuf};

#[derive(Clone, Debug, Parser)]
#[command(version, about)]
pub struct Config {
    #[arg(long, default_value = "127.0.0.1:8080")]
    pub listen: SocketAddr,
    #[arg(long)]
    pub data_dir: PathBuf,
    #[arg(long)]
    pub engine: PathBuf,
    #[arg(long, default_value_t = 0)]
    pub hip_device: usize,
    /// Override automatic batch selection (262144 for one exact WPA2 record, otherwise 4096).
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=262144))]
    pub gpu_batch: Option<u32>,
    #[arg(long)]
    pub gpu_pci: Option<String>,
    /// CPU execution for local integration tests.
    #[arg(long)]
    pub cpu: bool,
    #[arg(long, default_value_t = 64, value_parser = clap::value_parser!(u32).range(1..=10000))]
    pub max_jobs: u32,
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u32).range(1..=8))]
    pub max_upload_mib: u32,
}

impl Config {
    pub fn normalize(mut self) -> anyhow::Result<Self> {
        self.engine = self.engine.canonicalize()?;
        std::fs::create_dir_all(&self.data_dir)?;
        self.data_dir = self.data_dir.canonicalize()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.data_dir, std::fs::Permissions::from_mode(0o700))?;
        }
        for directory in ["jobs", "staging"] {
            std::fs::create_dir_all(self.data_dir.join(directory))?;
        }
        if let Some(address) = &self.gpu_pci {
            anyhow::ensure!(
                address.len() == 12
                    && address.bytes().enumerate().all(|(i, c)| match i {
                        4 | 7 => c == b':',
                        10 => c == b'.',
                        _ => c.is_ascii_hexdigit(),
                    }),
                "GPU PCI address must look like 0000:01:00.0"
            );
        }
        Ok(self)
    }
}
