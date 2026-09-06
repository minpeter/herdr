use super::*;
use std::ffi::OsString;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture {
    root: PathBuf,
    env: Vec<(&'static str, Option<OsString>)>,
    _lock: crate::integration::env::IntegrationEnvLock,
}

impl Fixture {
    fn new() -> Self {
        let lock = crate::integration::integration_env_lock();
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "herdr-senpi-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let env = [
            "HOME",
            "OMO_CODING_AGENT_DIR",
            "SENPI_CODING_AGENT_DIR",
            "PI_CODING_AGENT_DIR",
            "PI_CONFIG_DIR",
        ]
        .into_iter()
        .map(|key| {
            let value = std::env::var_os(key);
            std::env::remove_var(key);
            (key, value)
        })
        .collect();
        std::env::set_var("HOME", &root);
        Self {
            root,
            env,
            _lock: lock,
        }
    }

    fn agent(&self, relative: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(&path).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for (key, value) in &self.env {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn local_senpi_install_status_uninstall_and_pi_omp_coexistence() {
    let fixture = Fixture::new();
    std::env::set_var("OMO_CODING_AGENT_DIR", fixture.agent(".omo/agent"));
    std::env::set_var("SENPI_CODING_AGENT_DIR", fixture.agent(".senpi/agent"));
    fixture.agent(".pi/agent");
    fixture.agent(".omp/agent");
    let pi = crate::integration::targets::install_pi().unwrap();
    let omp = crate::integration::targets::install_omp()
        .unwrap()
        .extension_path;
    for brand in Brand::ALL {
        let path = fixture
            .root
            .join(brand.config_name())
            .join("agent/extensions")
            .join(INSTALL_NAME);
        assert_eq!(brand.path().unwrap(), path);
        assert_eq!(
            status(brand).unwrap().state,
            IntegrationStatusKind::NotInstalled
        );
        install(brand).unwrap();
        install(brand).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), ASSET);
        let current = status(brand).unwrap();
        assert_eq!(current.state, IntegrationStatusKind::Current);
        assert_eq!(current.installed_version, Some(VERSION));
        fs::write(
            &path,
            "// HERDR_INTEGRATION_ID=senpi\n// HERDR_INTEGRATION_VERSION=0\n",
        )
        .unwrap();
        assert_eq!(
            status(brand).unwrap().state,
            IntegrationStatusKind::Outdated
        );
        install(brand).unwrap();
        let user_file = path.with_file_name("user.ts");
        fs::write(&user_file, "user extension").unwrap();
        uninstall(brand).unwrap();
        uninstall(brand).unwrap();
        assert_eq!(
            status(brand).unwrap().state,
            IntegrationStatusKind::NotInstalled
        );
        assert_eq!(fs::read_to_string(user_file).unwrap(), "user extension");
    }
    assert_eq!(
        fs::read_to_string(pi).unwrap(),
        crate::integration::PI_EXTENSION_ASSET
    );
    assert_eq!(
        fs::read_to_string(omp).unwrap(),
        crate::integration::OMP_EXTENSION_ASSET
    );
}

#[test]
fn local_senpi_overrides_follow_defined_value_precedence() {
    let fixture = Fixture::new();
    let default_omo = Brand::Omo.path().unwrap();
    let default_senpi = Brand::Senpi.path().unwrap();
    let extension = |path: PathBuf| path.join("extensions").join(INSTALL_NAME);
    std::env::set_var("PI_CODING_AGENT_DIR", fixture.root.join("legacy"));
    for brand in Brand::ALL {
        assert_eq!(
            brand.path().unwrap(),
            extension(fixture.root.join("legacy"))
        );
    }
    std::env::set_var("SENPI_CODING_AGENT_DIR", "~/standalone");
    for brand in Brand::ALL {
        assert_eq!(
            brand.path().unwrap(),
            extension(fixture.root.join("standalone"))
        );
    }
    std::env::set_var("OMO_CODING_AGENT_DIR", "relative-agent");
    assert_eq!(
        Brand::Omo.path().unwrap(),
        extension(PathBuf::from("relative-agent"))
    );
    std::env::set_var("OMO_CODING_AGENT_DIR", "");
    assert_eq!(Brand::Omo.path().unwrap(), default_omo);
    std::env::set_var("SENPI_CODING_AGENT_DIR", "");
    assert_eq!(Brand::Senpi.path().unwrap(), default_senpi);
    std::env::set_var("OMO_CODING_AGENT_DIR", "~someone/agent");
    assert_eq!(
        Brand::Omo.path().unwrap(),
        extension(PathBuf::from("~someone/agent"))
    );
}

#[test]
fn local_senpi_project_override_is_nearest_with_home_boundary() {
    let fixture = Fixture::new();
    for brand in Brand::ALL {
        let outer = fixture.agent(&format!("project/{}/agent", brand.config_name()));
        let inner = fixture.agent(&format!("project/nested/{}/agent", brand.config_name()));
        let cwd = fixture.agent("project/nested/deeper");
        assert_eq!(project_agent_dir(brand, &cwd, &fixture.root), inner);
        fs::remove_dir(&inner).unwrap();
        assert_eq!(project_agent_dir(brand, &cwd, &fixture.root), outer);
        assert_eq!(
            project_agent_dir(brand, &fixture.root, &fixture.root),
            fixture.root.join(brand.config_name()).join("agent")
        );
    }
}

#[test]
fn local_senpi_refuses_unowned_files_and_pi_omp_collisions() {
    let fixture = Fixture::new();
    let agent = fixture.agent("custom/extensions");
    let path = agent.join(INSTALL_NAME);
    for content in [
        "user extension",
        "// HERDR_INTEGRATION_ID=senpi-other\n// HERDR_INTEGRATION_VERSION=1\n",
        crate::integration::PI_EXTENSION_ASSET,
    ] {
        fs::write(&path, content).unwrap();
        assert!(install_at(&path).is_err());
        assert!(uninstall_at(&path).is_err());
        assert!(status_at(path.clone()).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), content);
    }
    fs::remove_file(&path).unwrap();
    for name in [
        crate::integration::PI_EXTENSION_INSTALL_NAME,
        crate::integration::OMP_EXTENSION_INSTALL_NAME,
    ] {
        let other = agent.join(name);
        fs::write(&other, "keep").unwrap();
        assert!(install_at(&path).is_err());
        assert!(!path.exists());
        assert_eq!(fs::read_to_string(&other).unwrap(), "keep");
        fs::remove_file(other).unwrap();
    }
    let pi_agent = fixture.agent(".pi/agent");
    std::env::set_var("OMO_CODING_AGENT_DIR", pi_agent);
    assert!(install(Brand::Omo).is_err());
    let shared = fixture.agent("legacy");
    std::env::set_var("PI_CODING_AGENT_DIR", shared);
    assert!(install(Brand::Senpi).is_err());
}

#[cfg(unix)]
#[test]
fn local_senpi_refuses_file_symlinks_and_skips_project_symlinks() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let agent = fixture.agent("custom/extensions");
    let unrelated = fixture.root.join("unrelated.ts");
    fs::write(&unrelated, ASSET).unwrap();
    let path = agent.join(INSTALL_NAME);
    symlink(&unrelated, &path).unwrap();
    assert!(install_at(&path).is_err());
    assert!(uninstall_at(&path).is_err());
    assert_eq!(fs::read_to_string(&unrelated).unwrap(), ASSET);
    fs::remove_file(&unrelated).unwrap();
    assert!(install_at(&path).is_err());
    assert!(uninstall_at(&path).is_err());
    let other_agent = fixture.agent("other-agent");
    symlink(&agent, other_agent.join("extensions")).unwrap();
    let alias_path = other_agent.join("extensions").join(INSTALL_NAME);
    assert!(install_at(&alias_path).is_err());
    assert!(uninstall_at(&alias_path).is_err());
    let cwd = fixture.agent("project");
    fixture.agent("custom/agent");
    symlink(fixture.root.join("custom"), cwd.join(".omo")).unwrap();
    assert_eq!(
        project_agent_dir(Brand::Omo, &cwd, &fixture.root),
        fixture.root.join(".omo/agent")
    );
    let pi = fixture.agent(".pi/agent");
    let alias = fixture.root.join("pi-alias");
    symlink(pi, &alias).unwrap();
    std::env::set_var("OMO_CODING_AGENT_DIR", alias);
    assert!(install(Brand::Omo).is_err());
}

#[test]
fn local_senpi_requires_existing_agent_directory() {
    let fixture = Fixture::new();
    let path = fixture.root.join("missing/extensions").join(INSTALL_NAME);
    assert!(install_at(&path).is_err());
    assert!(!path.exists());
    assert!(!uninstall_at(&path).unwrap());
}
