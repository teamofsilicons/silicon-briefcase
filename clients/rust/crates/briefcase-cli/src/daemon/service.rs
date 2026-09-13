//! Login-service installation. Paths are encoded for each supervisor's grammar.
use super::{io, private_directory, root};
use crate::run::CliError;
use std::{path::PathBuf, process::Command};

const LABEL: &str = "com.teamofsilicons.briefcase";

fn home() -> Result<PathBuf, CliError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| CliError::Usage("HOME is required to install the login service".into()))
}

fn run(program: &str, args: &[&str]) -> Result<(), CliError> {
    let status = Command::new(program).args(args).status().map_err(io)?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::Usage(format!(
            "{program} failed with {status}; use `briefcase daemon run` under your supervisor"
        )))
    }
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn systemd(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}

fn definition(exe: &str, home: &str, runtime: &str, state: &str, path: &str, mac: bool) -> String {
    let toolchain = std::env::var("RUSTUP_TOOLCHAIN").unwrap_or_else(|_| "1.98.0".into());
    if mac {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{LABEL}</string><key>ProgramArguments</key><array><string>{}</string><string>daemon</string><string>run</string></array><key>EnvironmentVariables</key><dict><key>HOME</key><string>{}</string><key>BRIEFCASE_DAEMON_HOME</key><string>{}</string><key>BRIEFCASE_HOME</key><string>{}</string><key>PATH</key><string>{}</string><key>RUSTUP_TOOLCHAIN</key><string>{}</string></dict><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>10</integer><key>ExitTimeOut</key><integer>600</integer><key>StandardOutPath</key><string>{}/daemon.log</string><key>StandardErrorPath</key><string>{}/daemon.log</string></dict></plist>\n",
            xml(exe),
            xml(home),
            xml(runtime),
            xml(state),
            xml(path),
            xml(&toolchain),
            xml(runtime),
            xml(runtime)
        )
    } else {
        format!(
            "[Unit]\nDescription=Briefcase automatic updates\nAfter=network-online.target\n\n[Service]\nExecStart={} daemon run\nEnvironment={}\nEnvironment={}\nEnvironment={}\nEnvironment={}\nEnvironment={}\nEnvironment={}\nRestart=always\nRestartSec=10\nTimeoutStopSec=600\nUMask=0077\n\n[Install]\nWantedBy=default.target\n",
            systemd(exe),
            systemd(&format!("HOME={home}")),
            systemd(&format!("BRIEFCASE_DAEMON_HOME={runtime}")),
            systemd(&format!("BRIEFCASE_HOME={state}")),
            systemd(&format!("PATH={path}")),
            systemd("BRIEFCASE_AUTO_UPDATE="),
            systemd(&format!("RUSTUP_TOOLCHAIN={toolchain}"))
        )
    }
}

pub(super) fn install() -> Result<(), CliError> {
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        return Err(CliError::Usage("login service installation supports macOS launchd and Linux systemd; use `briefcase daemon run` with another supervisor".into()));
    }
    let home = home()?;
    let runtime = root()?;
    private_directory(&runtime)?;
    let state = crate::state::StateDirectory::locate()?;
    private_directory(state.path())?;
    let executable = std::env::current_exe().map_err(io)?;
    let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into());
    let mac = cfg!(target_os = "macos");
    let directory = if mac {
        home.join("Library/LaunchAgents")
    } else {
        home.join(".config/systemd/user")
    };
    std::fs::create_dir_all(&directory).map_err(io)?;
    let file = directory.join(if mac {
        format!("{LABEL}.plist")
    } else {
        "briefcase.service".into()
    });
    std::fs::write(
        &file,
        definition(
            &executable.to_string_lossy(),
            &home.to_string_lossy(),
            &runtime.to_string_lossy(),
            &state.path().to_string_lossy(),
            &path,
            mac,
        ),
    )
    .map_err(io)?;
    if mac {
        let uid = String::from_utf8(Command::new("id").arg("-u").output().map_err(io)?.stdout)
            .map_err(|_| CliError::Usage("cannot read current user ID".into()))?;
        let domain = format!("gui/{}", uid.trim());
        let _ = Command::new("launchctl")
            .args(["bootout", &format!("{domain}/{LABEL}")])
            .output();
        run(
            "launchctl",
            &["bootstrap", &domain, &file.to_string_lossy()],
        )
    } else {
        run("systemctl", &["--user", "daemon-reload"])?;
        run(
            "systemctl",
            &["--user", "enable", "--now", "briefcase.service"],
        )
    }
}

pub(super) fn uninstall() -> Result<(), CliError> {
    let home = home()?;
    let file = if cfg!(target_os = "macos") {
        let file = home.join(format!("Library/LaunchAgents/{LABEL}.plist"));
        if file.exists() {
            run("launchctl", &["unload", &file.to_string_lossy()])?;
        }
        file
    } else if cfg!(target_os = "linux") {
        let file = home.join(".config/systemd/user/briefcase.service");
        if file.exists() {
            run(
                "systemctl",
                &["--user", "disable", "--now", "briefcase.service"],
            )?;
        }
        file
    } else {
        return Ok(());
    };
    if file.exists() {
        std::fs::remove_file(file).map_err(io)?;
    }
    if cfg!(target_os = "linux") {
        run("systemctl", &["--user", "daemon-reload"])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn service_definitions_preserve_paths_and_do_not_interpolate_them() {
        let mac = definition("/Users/A & B/bin/briefcase", "/h", "/r", "/s", "/bin", true);
        assert!(mac.contains("A &amp; B"));
        assert!(!mac.contains("BRIEFCASE_TOKEN"));
        let linux = definition("/h/a%$\"b/briefcase", "/h", "/r", "/s", "/bin", false);
        assert!(linux.contains("ExecStart=\"/h/a%%$$\\\"b/briefcase\" daemon run"));
        assert!(linux.contains("Restart=always"));
    }
}
