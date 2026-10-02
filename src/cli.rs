use std::{ffi::OsString, path::PathBuf};

use crate::config::Config;

pub const HELP: &str = "network-monitor - record internet checks while the TUI is open\n\nUsage: network-monitor [--label LABEL] [--config PATH] [--db PATH]\n\n  --label LABEL  Optional ride label\n  --config PATH Read probe settings from a TOML file\n  --db PATH     Recording database (default: ~/Library/Application Support/network-monitor/rides.sqlite3)\n  --help        Show this help\n  --version     Show version\n\nPress q or Ctrl-C to finish the recording. An interactive terminal is required.";

#[derive(Debug, Default)]
pub struct Cli {
    pub label: Option<String>,
    pub config_path: Option<PathBuf>,
    pub database_path: Option<PathBuf>,
}

#[derive(Debug)]
pub enum ParseResult {
    Run(Cli),
    Help,
    Version,
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("{0}")]
    Argument(String),
    #[error("cannot read configuration {path}: {source}")]
    ReadConfig {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid TOML configuration: {0}")]
    Toml(#[from] toml::de::Error),
    #[error(transparent)]
    Config(#[from] crate::config::ConfigError),
    #[error("HOME is not set; supply --db PATH")]
    MissingHome,
}

impl Cli {
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<ParseResult, CliError> {
        let mut options = Self::default();
        let mut args = args.into_iter();
        while let Some(argument) = args.next() {
            let Some(name) = argument.to_str() else {
                return Err(CliError::Argument(
                    "option names must be valid UTF-8".into(),
                ));
            };
            match name {
                "--help" | "-h" => return Ok(ParseResult::Help),
                "--version" | "-V" => return Ok(ParseResult::Version),
                "--label" | "--config" | "--db" => {
                    let value = args
                        .next()
                        .ok_or_else(|| CliError::Argument(format!("{name} requires a value")))?;
                    match name {
                        "--label" => {
                            options.label = Some(value.into_string().map_err(|_| {
                                CliError::Argument("ride label must be valid UTF-8".into())
                            })?)
                        }
                        "--config" => options.config_path = Some(value.into()),
                        "--db" => options.database_path = Some(value.into()),
                        _ => return Err(CliError::Argument(format!("unknown option: {name}"))),
                    }
                }
                other => {
                    return Err(CliError::Argument(format!(
                        "unknown option: {other}; use --help"
                    )));
                }
            }
        }
        Ok(ParseResult::Run(options))
    }

    pub fn config(&self) -> Result<Config, CliError> {
        let config = match &self.config_path {
            Some(path) => toml::from_str(&std::fs::read_to_string(path).map_err(|source| {
                CliError::ReadConfig {
                    path: path.clone(),
                    source,
                }
            })?)?,
            None => Config::default(),
        };
        config.validate()?;
        Ok(config)
    }

    pub fn database(&self) -> Result<PathBuf, CliError> {
        match &self.database_path {
            Some(path) => Ok(path.clone()),
            None => Ok(
                PathBuf::from(std::env::var_os("HOME").ok_or(CliError::MissingHome)?)
                    .join("Library/Application Support/network-monitor/rides.sqlite3"),
            ),
        }
    }
}
