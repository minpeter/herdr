use crate::api::schema::IntegrationTarget;
use crate::integration::senpi::{self, Brand};

// Deliberately local: IntegrationTarget is append-closed in the v1 API.
#[derive(Debug, PartialEq, Eq)]
enum LocalTarget {
    Builtin(IntegrationCommandTarget),
    Senpi(Brand),
}

pub(super) fn run_integration_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(|arg| arg.as_str()) else {
        print_integration_help();
        return Ok(2);
    };

    match subcommand {
        "install" => integration_install(&args[1..]),
        "uninstall" => integration_uninstall(&args[1..]),
        "status" => integration_status(&args[1..]),
        "help" | "--help" | "-h" => {
            print_integration_help();
            Ok(0)
        }
        _ => {
            print_integration_help();
            Ok(2)
        }
    }
}

fn integration_status(args: &[String]) -> std::io::Result<i32> {
    let outdated_only = match args {
        [] => false,
        [flag] if flag == "--outdated-only" => true,
        _ => {
            eprintln!("usage: herdr integration status [--outdated-only]");
            return Ok(2);
        }
    };

    if outdated_only {
        crate::integration::print_outdated_update_notice();
        return print_local_senpi_statuses(true);
    }

    for status in crate::integration::installed_integration_statuses() {
        let target = crate::integration::integration_target_label(status.target);
        let state = describe_integration_state(
            status.state,
            status.installed_version,
            status.expected_version,
        );
        println!("{target}: {state} ({})", status.path.display());
    }

    if let Some(status) = crate::integration::experimental_letta_integration_status() {
        let state = describe_integration_state(
            status.state,
            status.installed_version,
            status.expected_version,
        );
        println!(
            "{} (experimental): {state} ({})",
            status.label,
            status.path.display()
        );
    }

    print_local_senpi_statuses(false)
}

fn print_local_senpi_statuses(outdated_only: bool) -> std::io::Result<i32> {
    let mut exit_code = 0;
    for brand in Brand::ALL {
        let status = match senpi::status(brand) {
            Ok(status) => status,
            Err(err) => {
                eprintln!("{}: {err}", brand.label());
                exit_code = 1;
                continue;
            }
        };
        let state = match status.state {
            crate::integration::IntegrationStatusKind::NotInstalled => "not installed".to_string(),
            crate::integration::IntegrationStatusKind::Current => {
                format!("current (v{})", senpi::VERSION)
            }
            crate::integration::IntegrationStatusKind::Outdated => format!(
                "needs update/repair (v{}; bundled v{}); run `herdr integration install {}`",
                status.installed_version.unwrap_or(0),
                senpi::VERSION,
                brand.label()
            ),
        };
        if !outdated_only || status.state == crate::integration::IntegrationStatusKind::Outdated {
            println!("{}: {state} ({})", brand.label(), status.path.display());
        }
    }
    Ok(exit_code)
}

fn describe_integration_state(
    state: crate::integration::IntegrationStatusKind,
    installed_version: Option<u32>,
    expected_version: u32,
) -> String {
    let version = match installed_version {
        Some(version) => format!("v{version}"),
        None => "legacy".to_string(),
    };
    match state {
        crate::integration::IntegrationStatusKind::NotInstalled => "not installed".to_string(),
        crate::integration::IntegrationStatusKind::Current => format!("current ({version})"),
        crate::integration::IntegrationStatusKind::Outdated
            if installed_version.is_some_and(|installed| installed >= expected_version) =>
        {
            format!("needs repair ({version})")
        }
        crate::integration::IntegrationStatusKind::Outdated => {
            format!("outdated ({version} < v{expected_version})")
        }
    }
}

fn integration_install(args: &[String]) -> std::io::Result<i32> {
    let Some(target) = parse_integration_target(args, "install")? else {
        return Ok(2);
    };

    let result = match target {
        LocalTarget::Builtin(IntegrationCommandTarget::Builtin(target)) => {
            crate::integration::install_target(target)
        }
        LocalTarget::Builtin(IntegrationCommandTarget::Letta) => {
            crate::integration::install_experimental_letta()
        }
        LocalTarget::Senpi(brand) => senpi::install(brand),
    };
    match result {
        Ok(messages) => {
            print_integration_messages(messages);
            Ok(0)
        }
        Err(err) => {
            eprintln!("{err}");
            Ok(1)
        }
    }
}

fn integration_uninstall(args: &[String]) -> std::io::Result<i32> {
    let Some(target) = parse_integration_target(args, "uninstall")? else {
        return Ok(2);
    };

    let result = match target {
        LocalTarget::Builtin(IntegrationCommandTarget::Builtin(target)) => {
            crate::integration::uninstall_target(target)
        }
        LocalTarget::Builtin(IntegrationCommandTarget::Letta) => {
            crate::integration::uninstall_experimental_letta()
        }
        LocalTarget::Senpi(brand) => senpi::uninstall(brand),
    };
    match result {
        Ok(messages) => {
            print_integration_messages(messages);
            Ok(0)
        }
        Err(err) => {
            eprintln!("{err}");
            Ok(1)
        }
    }
}

fn print_integration_messages(messages: Vec<String>) {
    for message in messages {
        println!("{message}");
    }
}

/// Integration target accepted by the CLI. Letta is deliberately kept out of
/// the frozen client endpoint `IntegrationTarget` enum and is handled as an
/// experimental CLI-only target until the agent registry replaces it.
enum IntegrationCommandTarget {
    Builtin(IntegrationTarget),
    Letta,
}

fn parse_integration_target(args: &[String], action: &str) -> std::io::Result<Option<LocalTarget>> {
    let Some(target) = args.first().map(|arg| arg.as_str()) else {
        eprintln!(
            "usage: herdr integration {action} <pi|omp|omo|senpi|claude|codex|copilot|devin|droid|kimi|opencode|kilo|hermes|qodercli|qwen|letta|cursor|mastracode|grok>"
        );
        return Ok(None);
    };
    if args.len() != 1 {
        eprintln!(
            "usage: herdr integration {action} <pi|omp|omo|senpi|claude|codex|copilot|devin|droid|kimi|opencode|kilo|hermes|qodercli|qwen|letta|cursor|mastracode|grok>"
        );
        return Ok(None);
    }

    let brand = match target {
        "omo" => Some(Brand::Omo),
        "senpi" => Some(Brand::Senpi),
        _ => None,
    };
    if let Some(brand) = brand {
        return Ok(Some(LocalTarget::Senpi(brand)));
    }

    let parsed = match target {
        "pi" => IntegrationCommandTarget::Builtin(IntegrationTarget::Pi),
        "omp" => IntegrationCommandTarget::Builtin(IntegrationTarget::Omp),
        "claude" => IntegrationCommandTarget::Builtin(IntegrationTarget::Claude),
        "codex" => IntegrationCommandTarget::Builtin(IntegrationTarget::Codex),
        "copilot" => IntegrationCommandTarget::Builtin(IntegrationTarget::Copilot),
        "devin" => IntegrationCommandTarget::Builtin(IntegrationTarget::Devin),
        "droid" => IntegrationCommandTarget::Builtin(IntegrationTarget::Droid),
        "kimi" => IntegrationCommandTarget::Builtin(IntegrationTarget::Kimi),
        "opencode" => IntegrationCommandTarget::Builtin(IntegrationTarget::Opencode),
        "kilo" => IntegrationCommandTarget::Builtin(IntegrationTarget::Kilo),
        "hermes" => IntegrationCommandTarget::Builtin(IntegrationTarget::Hermes),
        "qodercli" => IntegrationCommandTarget::Builtin(IntegrationTarget::Qodercli),
        "qwen" => IntegrationCommandTarget::Builtin(IntegrationTarget::Qwen),
        "letta" => IntegrationCommandTarget::Letta,
        "cursor" => IntegrationCommandTarget::Builtin(IntegrationTarget::Cursor),
        "mastracode" => IntegrationCommandTarget::Builtin(IntegrationTarget::Mastracode),
        "antigravity-cli" | "antigravity_cli" => {
            IntegrationCommandTarget::Builtin(IntegrationTarget::AntigravityCli)
        }
        "grok" => IntegrationCommandTarget::Builtin(IntegrationTarget::Grok),
        _ => {
            eprintln!("unknown integration target: {target}");
            eprintln!(
                "currently supported: pi, omp, omo, senpi, claude, codex, copilot, devin, droid, kimi, opencode, kilo, hermes, qodercli, qwen, letta, cursor, mastracode, antigravity-cli, grok"
            );
            return Ok(None);
        }
    };

    Ok(Some(LocalTarget::Builtin(parsed)))
}

fn print_integration_help() {
    eprintln!("herdr integration commands:");
    eprintln!("  herdr integration install pi");
    eprintln!("  herdr integration install omp");
    eprintln!("  herdr integration install omo|senpi (local only)");
    eprintln!("  herdr integration install claude");
    eprintln!("  herdr integration install codex");
    eprintln!("  herdr integration install copilot");
    eprintln!("  herdr integration install devin");
    eprintln!("  herdr integration install droid");
    eprintln!("  herdr integration install kimi");
    eprintln!("  herdr integration install opencode");
    eprintln!("  herdr integration install kilo");
    eprintln!("  herdr integration install hermes");
    eprintln!("  herdr integration install qodercli");
    eprintln!("  herdr integration install qwen");
    eprintln!("  herdr integration install letta");
    eprintln!("  herdr integration install cursor");
    eprintln!("  herdr integration install mastracode");
    eprintln!("  herdr integration install antigravity-cli");
    eprintln!("  herdr integration install grok");
    eprintln!("  herdr integration uninstall pi");
    eprintln!("  herdr integration uninstall omp");
    eprintln!("  herdr integration uninstall omo|senpi (local only)");
    eprintln!("  herdr integration uninstall claude");
    eprintln!("  herdr integration uninstall codex");
    eprintln!("  herdr integration uninstall copilot");
    eprintln!("  herdr integration uninstall devin");
    eprintln!("  herdr integration uninstall droid");
    eprintln!("  herdr integration uninstall kimi");
    eprintln!("  herdr integration uninstall opencode");
    eprintln!("  herdr integration uninstall kilo");
    eprintln!("  herdr integration uninstall hermes");
    eprintln!("  herdr integration uninstall qodercli");
    eprintln!("  herdr integration uninstall qwen");
    eprintln!("  herdr integration uninstall letta");
    eprintln!("  herdr integration uninstall cursor");
    eprintln!("  herdr integration uninstall mastracode");
    eprintln!("  herdr integration uninstall antigravity-cli");
    eprintln!("  herdr integration uninstall grok");
    eprintln!("  herdr integration status [--outdated-only]");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_senpi_targets_are_accepted() {
        for target in ["omo", "senpi"] {
            for action in ["install", "uninstall"] {
                assert!(
                    parse_integration_target(&[target.into()], action)
                        .unwrap()
                        .is_some(),
                    "{action} {target}"
                );
            }
        }
    }

    #[test]
    fn local_senpi_routing_preserves_legacy_targets_and_usage_errors() {
        for target in IntegrationTarget::ALL {
            let label = crate::integration::integration_target_label(target);
            assert_eq!(
                parse_integration_target(&[label.into()], "install").unwrap(),
                Some(LocalTarget::Shared(target))
            );
        }
        for target in ["omo", "senpi"] {
            assert!(
                parse_integration_target(&[target.into(), "extra".into()], "install")
                    .unwrap()
                    .is_none()
            );
        }
        assert!(parse_integration_target(&[], "install").unwrap().is_none());
        assert!(parse_integration_target(&["unknown".into()], "install")
            .unwrap()
            .is_none());
    }
}
