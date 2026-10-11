use chrono::{TimeDelta, Utc};
use network_monitor::{
    config::Config,
    model::{
        Event, LocationEvent, LocationState, Monitor, Observation, ProbeKind, ProbeOutcome, Stamp,
    },
    storage::{Recorder, Session},
    ui::View,
    web,
};
use std::{path::PathBuf, time::Duration};
use tokio::sync::watch;

fn event(
    elapsed_ms: u64,
    start: chrono::DateTime<Utc>,
    kind: ProbeKind,
    target: String,
    failure: bool,
) -> Event {
    Event::Probe(Observation {
        at: Stamp {
            utc: start + TimeDelta::milliseconds(elapsed_ms as i64),
            elapsed_ms,
            generation: 0,
        },
        started_elapsed_ms: elapsed_ms.saturating_sub(42),
        duration_ms: 42,
        kind,
        target,
        outcome: if failure {
            ProbeOutcome::Timeout
        } else {
            ProbeOutcome::Success
        },
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let database = temp.path().join("fixture.sqlite3");
    let config = Config::default();
    let start = Utc::now() - TimeDelta::seconds(120);
    let mut session = Session::new(Some("SIMULATED browser validation".into()), &config)?;
    session.started_utc = start;
    let recorder = Recorder::open(&database, session.clone()).await?;
    let mut monitor = Monitor::new(config.clone());
    for second in (0..=120).step_by(2) {
        let elapsed_ms = second * 1000;
        if !(52..70).contains(&second) {
            let location = Event::Location(LocationEvent {
                at: Stamp {
                    utc: start + TimeDelta::seconds(second as i64),
                    elapsed_ms,
                    generation: 0,
                },
                state: LocationState::Fix {
                    latitude: 52.48 - second as f64 / 120.0 * 1.13,
                    longitude: 13.36 - second as f64 / 120.0 * 0.99,
                    horizontal_accuracy_m: 60.0,
                    source_utc: start + TimeDelta::seconds(second as i64),
                },
            });
            monitor.apply(location.clone());
            recorder.record(location).await?;
        } else if second == 52 {
            let location = Event::Location(LocationEvent {
                at: Stamp {
                    utc: start + TimeDelta::seconds(second as i64),
                    elapsed_ms,
                    generation: 0,
                },
                state: LocationState::Unavailable {
                    reason: "Fixture location gap".into(),
                },
            });
            monitor.apply(location.clone());
            recorder.record(location).await?;
        }
        for (kind, target) in [
            (ProbeKind::Tcp, "1.1.1.1:443"),
            (ProbeKind::Tcp, "8.8.8.8:443"),
            (ProbeKind::Dns, "example.com"),
            (ProbeKind::Https, "https://www.gstatic.com/generate_204"),
        ] {
            let observation = event(
                elapsed_ms,
                start,
                kind,
                target.into(),
                (34..40).contains(&second),
            );
            monitor.apply(observation.clone());
            recorder.record(observation).await?;
        }
    }
    let (sender, receiver) = watch::channel(None);
    let server = web::start(
        8742,
        database.clone(),
        receiver,
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("web/dist"),
    )
    .await?;
    println!(
        "SIMULATED fixture server: http://{}; Ctrl-C stops it",
        server.address
    );
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    let mut elapsed_ms = 120_000;
    loop {
        tokio::select! {
            signal=tokio::signal::ctrl_c()=>{signal?;break;},
            _tick=interval.tick()=>{
                elapsed_ms+=1000;
                for (kind,target) in [(ProbeKind::Tcp,"1.1.1.1:443"),(ProbeKind::Tcp,"8.8.8.8:443"),(ProbeKind::Dns,"example.com"),(ProbeKind::Https,"https://www.gstatic.com/generate_204")] {
                    let observation=event(elapsed_ms,start,kind,target.into(),false);
                    monitor.apply(observation.clone());recorder.record(observation).await?;
                }
                sender.send_replace(Some(View {snapshot:monitor.snapshot(elapsed_ms),config:config.clone(),utc:start+TimeDelta::milliseconds(elapsed_ms as i64),elapsed_ms,session_id:session.id.clone(),label:session.label.clone(),database:database.clone()}));
            }
        }
    }
    server.shutdown().await?;
    recorder.finish(Utc::now()).await?;
    Ok(())
}
