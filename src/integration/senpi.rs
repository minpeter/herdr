//! Local CLI packaging only: these brands must not enter the frozen API target enum.
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::env::{expand_tilde_path, home_dir, omp_extension_dir, pi_extension_dir};
use super::file_ops::remove_file_if_exists;
use super::registry::parse_integration_version;
use super::IntegrationStatusKind;

const INSTALL_NAME: &str = "herdr-senpi-agent-state.ts";
const ASSET: &str = include_str!("assets/senpi/herdr-agent-state.ts");
pub(crate) const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Brand {
    Omo,
    Senpi,
}

impl Brand {
    pub(crate) const ALL: [Self; 2] = [Self::Omo, Self::Senpi];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Omo => "omo",
            Self::Senpi => "senpi",
        }
    }

    const fn config_name(self) -> &'static str {
        match self {
            Self::Omo => ".omo",
            Self::Senpi => ".senpi",
        }
    }

    const fn env_names(self) -> &'static [&'static str] {
        match self {
            Self::Omo => &[
                "OMO_CODING_AGENT_DIR",
                "SENPI_CODING_AGENT_DIR",
                "PI_CODING_AGENT_DIR",
            ],
            Self::Senpi => &["SENPI_CODING_AGENT_DIR", "PI_CODING_AGENT_DIR"],
        }
    }

    pub(crate) fn path(self) -> io::Result<PathBuf> {
        // Senpi envValue uses the first DEFINED value, including empty values.
        let override_dir = self.env_names().iter().find_map(std::env::var_os);
        let agent_dir = if let Some(value) = override_dir.filter(|value| !value.is_empty()) {
            let path = PathBuf::from(value);
            let raw = path.to_string_lossy();
            if raw.starts_with("file://") {
                return Err(io::Error::other(
                    "use a native filesystem path for the agent directory, not a file URL",
                ));
            }
            let expand = raw == "~" || raw.starts_with("~/");
            #[cfg(windows)]
            let expand = expand || raw.starts_with("~\\");
            if expand {
                expand_tilde_path(path)?
            } else {
                path
            }
        } else {
            project_agent_dir(self, &std::env::current_dir()?, &home_dir()?)
        };
        Ok(agent_dir.join("extensions").join(INSTALL_NAME))
    }
}

// Mirrors Senpi 4df67dc87 resolveAgentDir/findNearestParentConfigDir:
// home excluded, depth 0..=100, neither config nor agent may be a symlink.
fn project_agent_dir(brand: Brand, cwd: &Path, home: &Path) -> PathBuf {
    for current in cwd.ancestors().take(101) {
        if current == home {
            break;
        }
        let config = current.join(brand.config_name());
        let agent = config.join("agent");
        if [config, agent.clone()]
            .iter()
            .all(|path| fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir()))
        {
            return agent;
        }
    }
    home.join(brand.config_name()).join("agent")
}

pub(crate) struct Status {
    pub path: PathBuf,
    pub state: IntegrationStatusKind,
    pub installed_version: Option<u32>,
}

pub(crate) fn status(brand: Brand) -> io::Result<Status> {
    status_at(brand.path()?)
}

fn status_at(path: PathBuf) -> io::Result<Status> {
    let content = owned_content(&path)?;
    let installed_version = content.as_deref().and_then(parse_integration_version);
    let state = match content.as_deref() {
        None => IntegrationStatusKind::NotInstalled,
        Some(content) if content == ASSET => IntegrationStatusKind::Current,
        Some(_) => IntegrationStatusKind::Outdated,
    };
    Ok(Status {
        path,
        state,
        installed_version,
    })
}

fn owned_content(path: &Path) -> io::Result<Option<String>> {
    // Do not follow an extensions-directory alias into another agent's files.
    if let Some(dir) = path.parent() {
        match fs::symlink_metadata(dir) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(io::Error::other(format!(
                    "refusing symlinked extensions directory at {}",
                    dir.display()
                )));
            }
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    if metadata.is_file() {
        let content = fs::read_to_string(path)?;
        if content
            .lines()
            .any(|line| line.trim() == "// HERDR_INTEGRATION_ID=senpi")
            && parse_integration_version(&content).is_some()
        {
            return Ok(Some(content));
        }
    }
    Err(io::Error::other(format!(
        "refusing unrelated or symlinked integration file at {}",
        path.display()
    )))
}

pub(crate) fn install(brand: Brand) -> io::Result<Vec<String>> {
    let path = brand.path()?;
    let agent_dir = path
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| io::Error::other("missing agent directory"))?;
    let canonical_agent = fs::canonicalize(agent_dir)?;
    // Legacy aliases can resolve to Pi/OMP's active directory. Never put a
    // Senpi-only extension there, even when the installed filenames differ.
    for other in [pi_extension_dir()?, omp_extension_dir()?] {
        let Some(other_agent) = other.parent() else {
            continue;
        };
        let other_agent = match fs::canonicalize(other_agent) {
            Ok(dir) => dir,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        if other_agent == canonical_agent {
            return Err(io::Error::other(format!("{} and Pi/OMP resolve to the same agent directory; configure separate agent directories", brand.label())));
        }
    }
    install_at(&path)?;
    Ok(vec![format!(
        "installed {} integration to {}",
        brand.label(),
        path.display()
    )])
}

fn install_at(path: &Path) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("missing extension directory"))?;
    if !dir.parent().is_some_and(Path::is_dir) {
        return Err(io::Error::other(format!(
            "agent directory not found at {}; install the agent first",
            dir.display()
        )));
    }
    for name in [
        super::PI_EXTENSION_INSTALL_NAME,
        super::OMP_EXTENSION_INSTALL_NAME,
    ] {
        match fs::symlink_metadata(dir.join(name)) {
            Ok(_) => {
                return Err(io::Error::other(format!(
                    "Pi/OMP extension already exists at {}; configure separate agent directories",
                    dir.display()
                )))
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    let content = owned_content(path)?;
    fs::create_dir_all(dir)?;
    if content.as_deref() != Some(ASSET) {
        match content {
            Some(_) => fs::write(path, ASSET)?,
            None => OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?
                .write_all(ASSET.as_bytes())?,
        }
    }
    Ok(())
}

pub(crate) fn uninstall(brand: Brand) -> io::Result<Vec<String>> {
    let path = brand.path()?;
    let removed = uninstall_at(&path)?;
    Ok(vec![format!(
        "{} {} integration at {}",
        if removed { "removed" } else { "no installed" },
        brand.label(),
        path.display()
    )])
}

fn uninstall_at(path: &Path) -> io::Result<bool> {
    if owned_content(path)?.is_none() {
        return Ok(false);
    }
    remove_file_if_exists(path)
}

#[cfg(test)]
mod tests;
