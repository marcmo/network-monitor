use network_monitor::cli::{Cli, ParseResult};
use std::{ffi::OsString, path::PathBuf, process::Command};

#[test]
fn approved_options_preserve_label_config_and_database_paths() {
    let args = [
        "--label",
        "Berlin ride",
        "--config",
        "ride.toml",
        "--db",
        "ride.sqlite3",
    ]
    .map(OsString::from);
    let ParseResult::Run(options) = Cli::parse(args).unwrap() else {
        panic!("expected launch");
    };
    assert_eq!(options.label.as_deref(), Some("Berlin ride"));
    assert_eq!(options.config_path, Some(PathBuf::from("ride.toml")));
    assert_eq!(options.database_path, Some(PathBuf::from("ride.sqlite3")));
    assert!(Cli::parse([OsString::from("--daemon")]).is_err());
    assert!(Cli::parse([OsString::from("--label")]).is_err());
}

#[test]
fn redirected_launch_rejects_before_creating_a_recording() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("not-created.sqlite3");
    let output = Command::new(env!("CARGO_BIN_EXE_network-monitor"))
        .arg("--db")
        .arg(&database)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal"));
    assert!(!database.exists());
    assert!(!output.stdout.contains(&27));
}

#[test]
fn help_is_available_without_a_terminal() {
    let output = Command::new(env!("CARGO_BIN_EXE_network-monitor"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("--label"));
    assert!(!output.stdout.contains(&27));
}
