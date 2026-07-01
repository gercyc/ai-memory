//! Detached native process launcher for background hook-spool drains.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Unit-testable description of the hidden drainer command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrainCommandSpec {
    /// Executable path.
    pub exe: PathBuf,
    /// Command arguments.
    pub args: Vec<OsString>,
    /// Stderr log file under `<data_dir>/logs`.
    pub stderr_log: PathBuf,
}

/// Unit-testable stdio/detach configuration for a drainer process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnConfig {
    /// stdin is redirected to null.
    pub stdin_null: bool,
    /// stdout is redirected to null.
    pub stdout_null: bool,
    /// stderr is redirected to this file.
    pub stderr_file: PathBuf,
    /// Platform detach flags applied to the command.
    pub detach: DetachConfig,
}

/// Platform detach configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetachConfig {
    /// Unix starts a new process group.
    #[cfg(unix)]
    UnixProcessGroupZero,
    /// Windows creation flags.
    #[cfg(windows)]
    WindowsCreationFlags(u32),
    /// No extra detach flags on unsupported platforms.
    #[cfg(not(any(unix, windows)))]
    None,
}

/// Build `ai-memory --data-dir <dir> hook-drain`.
pub fn command_spec(data_dir: &Path) -> io::Result<DrainCommandSpec> {
    Ok(DrainCommandSpec {
        exe: std::env::current_exe()?,
        args: vec![
            OsString::from("--data-dir"),
            data_dir.as_os_str().to_os_string(),
            OsString::from("hook-drain"),
        ],
        stderr_log: data_dir.join("logs").join("hook-drain.log"),
    })
}

/// Build the stdio/detach shape used when spawning.
#[must_use]
pub fn spawn_config(spec: &DrainCommandSpec, try_breakaway: bool) -> SpawnConfig {
    SpawnConfig {
        stdin_null: true,
        stdout_null: true,
        stderr_file: spec.stderr_log.clone(),
        detach: detach_config(try_breakaway),
    }
}

/// Spawn the hidden drainer without inheriting hook stdio.
pub fn spawn(data_dir: &Path) -> io::Result<()> {
    let spec = command_spec(data_dir)?;
    spawn_spec(&spec)
}

fn spawn_spec(spec: &DrainCommandSpec) -> io::Result<()> {
    if let Some(parent) = spec.stderr_log.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let config = spawn_config(spec, true);
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&config.stderr_file)?;

    let mut command = Command::new(&spec.exe);
    command
        .args(&spec.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr));
    apply_detach_flags(&mut command, &config.detach);

    match command.spawn() {
        Ok(_child) => Ok(()),
        Err(err) if should_retry_without_breakaway(&err) => {
            let mut log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&spec.stderr_log)?;
            let _ = writeln!(
                log,
                "ai-memory hook-drain: breakaway spawn failed with access denied; retrying without breakaway"
            );
            let config = spawn_config(spec, false);
            let stderr = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&config.stderr_file)?;
            let mut retry = Command::new(&spec.exe);
            retry
                .args(&spec.args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::from(stderr));
            apply_detach_flags(&mut retry, &config.detach);
            retry.spawn().map(|_| ())
        }
        Err(err) => Err(err),
    }
}

#[cfg(unix)]
fn detach_config(_try_breakaway: bool) -> DetachConfig {
    DetachConfig::UnixProcessGroupZero
}

#[cfg(unix)]
fn apply_detach_flags(command: &mut Command, detach: &DetachConfig) {
    use std::os::unix::process::CommandExt as _;
    if matches!(detach, DetachConfig::UnixProcessGroupZero) {
        command.process_group(0);
    }
}

#[cfg(windows)]
fn detach_config(try_breakaway: bool) -> DetachConfig {
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

    let mut flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;
    if try_breakaway {
        flags |= CREATE_BREAKAWAY_FROM_JOB;
    }
    DetachConfig::WindowsCreationFlags(flags)
}

#[cfg(windows)]
fn apply_detach_flags(command: &mut Command, detach: &DetachConfig) {
    use std::os::windows::process::CommandExt as _;
    if let DetachConfig::WindowsCreationFlags(flags) = detach {
        command.creation_flags(*flags);
    }
}

#[cfg(not(any(unix, windows)))]
fn detach_config(_try_breakaway: bool) -> DetachConfig {
    DetachConfig::None
}

#[cfg(not(any(unix, windows)))]
fn apply_detach_flags(_command: &mut Command, _detach: &DetachConfig) {}

#[cfg(not(windows))]
fn should_retry_without_breakaway(_err: &io::Error) -> bool {
    false
}

#[cfg(windows)]
fn should_retry_without_breakaway(err: &io::Error) -> bool {
    err.raw_os_error() == Some(5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_spec_uses_data_dir_then_hidden_subcommand() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = command_spec(tmp.path()).unwrap();

        assert_eq!(
            spec.args,
            vec![
                OsString::from("--data-dir"),
                tmp.path().as_os_str().to_os_string(),
                OsString::from("hook-drain"),
            ]
        );
        assert_eq!(
            spec.stderr_log,
            tmp.path().join("logs").join("hook-drain.log")
        );
    }

    #[test]
    fn spawn_config_redirects_stdio_and_logs_under_data_dir_logs() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = command_spec(tmp.path()).unwrap();
        let config = spawn_config(&spec, true);

        assert!(config.stdin_null);
        assert!(config.stdout_null);
        assert_eq!(
            config.stderr_file,
            tmp.path().join("logs").join("hook-drain.log")
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_spawn_config_uses_process_group_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = command_spec(tmp.path()).unwrap();
        assert_eq!(
            spawn_config(&spec, true).detach,
            DetachConfig::UnixProcessGroupZero
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_spawn_config_uses_expected_flags_and_breakaway_toggle() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = command_spec(tmp.path()).unwrap();
        let DetachConfig::WindowsCreationFlags(with_breakaway) = spawn_config(&spec, true).detach
        else {
            panic!("windows flags")
        };
        let DetachConfig::WindowsCreationFlags(without_breakaway) =
            spawn_config(&spec, false).detach
        else {
            panic!("windows flags")
        };
        assert_ne!(with_breakaway, without_breakaway);
    }
}
