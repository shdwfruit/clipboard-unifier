//! On-disk configuration (`config.toml` in the platform config directory).

use crate::crypto::Secret;
use crate::proto::DeviceId;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_PORT: u16 = 47321;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    /// Pairing code; contains the group secret. Keep this file private.
    pub code: String,
    /// Random per-machine identifier (hex).
    pub device_id: String,
    /// Friendly name shown to peers.
    pub name: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

impl Config {
    pub fn new(secret: &Secret, name: Option<String>) -> Self {
        Self {
            code: secret.to_code(),
            device_id: hex::encode(new_device_id()),
            name: name.unwrap_or_else(default_name),
            port: DEFAULT_PORT,
        }
    }

    pub fn secret(&self) -> Result<Secret> {
        Secret::from_code(&self.code)
    }

    pub fn device_id(&self) -> Result<DeviceId> {
        let bytes = hex::decode(&self.device_id).context("device_id is not hex")?;
        DeviceId::try_from(bytes).map_err(|_| anyhow::anyhow!("device_id must be 16 bytes"))
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self, path: &Path, overwrite: bool) -> Result<()> {
        if path.exists() && !overwrite {
            bail!(
                "{} already exists (use --force to replace it)",
                path.display()
            );
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        restrict_permissions(path)?;
        Ok(())
    }
}

pub fn default_path() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "clipbridge")
        .context("could not determine a config directory")?;
    Ok(dirs.config_dir().join("config.toml"))
}

pub fn new_device_id() -> DeviceId {
    rand::random()
}

pub fn default_name() -> String {
    gethostname::gethostname()
        .to_string_lossy()
        .trim_end_matches(".local")
        .to_string()
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<()> {
    Ok(())
}
