use std::{io::IsTerminal, os::unix::net::UnixStream, thread};

use network_monitor::{
    app::{AppOptions, AppPorts, Control},
    cli::{Cli, CliError, HELP, ParseResult},
    location,
    probe::{NetworkProbes, ProbeSetupError},
    scheduler::{EVENT_QUEUE_CAPACITY, SamplerError, SystemClock},
};
use tokio::sync::{mpsc, oneshot, watch};

#[derive(Debug, thiserror::Error)]
enum LaunchError {
    #[error(transparent)]
    Cli(#[from] CliError),
    #[error("an interactive terminal is required for stdin and stdout")]
    NonTerminal,
    #[error("application setup failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sampler(#[from] SamplerError),
    #[error(transparent)]
    Probes(#[from] ProbeSetupError),
    #[error(transparent)]
    App(#[from] network_monitor::app::AppError),
    #[error("{0}")]
    Worker(String),
}

fn main() -> std::process::ExitCode {
    match launch() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("network-monitor: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn launch() -> Result<(), LaunchError> {
    let options = match Cli::parse(std::env::args_os().skip(1))? {
        ParseResult::Help => {
            println!("{HELP}");
            return Ok(());
        }
        ParseResult::Version => {
            println!("network-monitor {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        ParseResult::Run(options) => options,
    };
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(LaunchError::NonTerminal);
    }
    let config = options.config()?;
    let database = options.database()?;
    let options = AppOptions {
        config,
        database,
        label: options.label,
    };
    let clock = SystemClock::new()?;
    let (locations_tx, locations) = mpsc::channel(EVENT_QUEUE_CAPACITY);
    let (native_tx, location_result) = oneshot::channel();
    let (stop_main, stop_worker) = UnixStream::pair()?;
    let (panic_tx, panic_rx) = std::sync::mpsc::sync_channel(4);
    // Panics are reported only after the terminal owner's unwind restores the screen.
    std::panic::set_hook(Box::new(move |info| {
        let _ = panic_tx.try_send(info.to_string());
    }));
    let worker = thread::Builder::new()
        .name("monitor".into())
        .spawn(move || {
            let _stop = StopMain(stop_worker);
            run_worker(options, clock, locations, location_result)
        })?;
    let native_result = location::run_main(locations_tx, stop_main);
    let _ = native_tx.send(native_result);
    let result = worker
        .join()
        .map_err(|_| LaunchError::Worker("application worker panicked".into()))
        .and_then(|result| result);
    for diagnostic in panic_rx.try_iter() {
        eprintln!("network-monitor panic: {diagnostic}");
    }
    result
}

struct StopMain(UnixStream);
impl Drop for StopMain {
    fn drop(&mut self) {
        let _ = self.0.shutdown(std::net::Shutdown::Write);
    }
}

fn run_worker(
    options: AppOptions,
    clock: SystemClock,
    locations: mpsc::Receiver<location::RawLocationEvent>,
    location_result: oneshot::Receiver<Result<(), location::LocationError>>,
) -> Result<(), LaunchError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let runner = NetworkProbes::new(&options.config)?;
    let (controls, control_rx) = mpsc::channel(8);
    let (views, view_rx) = watch::channel(None);
    let signal_controls = controls.clone();
    let signals = runtime.block_on(async move {
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
        Ok::<_, std::io::Error>(tokio::spawn(async move {
            tokio::select! {
                _ = interrupt.recv() => {},
                _ = terminate.recv() => {},
                _ = hangup.recv() => {},
            }
            let _ = signal_controls.send(Control::Quit).await;
        }))
    })?;
    let terminal = thread::Builder::new()
        .name("terminal".into())
        .spawn(move || {
            let terminal_controls = controls.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                network_monitor::terminal::run(view_rx, terminal_controls)
            }));
            let result = match result {
                Ok(result) => result.map_err(|error| error.to_string()),
                Err(_) => Err("terminal thread panicked".into()),
            };
            if let Err(error) = &result {
                let _ = controls.try_send(Control::TerminalFailed(error.clone()));
            }
            result
        })?;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(network_monitor::app::run_session(
            options,
            runner,
            clock,
            AppPorts {
                controls: control_rx,
                locations,
                location_result,
                views,
            },
        ))
    }));
    signals.abort();
    let terminal_result = terminal
        .join()
        .map_err(|_| LaunchError::Worker("terminal thread panicked".into()))?
        .map_err(LaunchError::Worker);
    match result {
        Ok(result) => result.map_err(LaunchError::from).and(terminal_result),
        Err(_) => Err(LaunchError::Worker("application worker panicked".into())),
    }
}
