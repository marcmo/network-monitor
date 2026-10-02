use chrono::Utc;
use network_monitor::{config::Config, model::*, storage::*};

#[tokio::test]
async fn separate_launches_persist_full_rides_and_source_location_time() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rides.sqlite3");
    let session = Session::new(Some("Hamburg".into()), &Config::default()).unwrap();
    let id = session.id.clone();
    let recorder = Recorder::open(&path, session).await.unwrap();
    let source_utc = Utc::now() - chrono::Duration::seconds(90);
    for elapsed_ms in [0, 600_000, 3_600_000] {
        recorder
            .record(Event::Location(LocationEvent {
                at: Stamp {
                    utc: Utc::now(),
                    elapsed_ms,
                    generation: 0,
                },
                state: LocationState::Fix {
                    latitude: 53.5,
                    longitude: 10.0,
                    horizontal_accuracy_m: 80.0,
                    source_utc,
                },
            }))
            .await
            .unwrap();
    }
    recorder.finish(Utc::now()).await.unwrap();
    Recorder::open(&path, Session::new(None, &Config::default()).unwrap())
        .await
        .unwrap()
        .finish(Utc::now())
        .await
        .unwrap();
    let sessions = read_sessions(&path).unwrap();
    assert_eq!(sessions.len(), 2);
    let recorded = sessions.iter().find(|value| value.id == id).unwrap();
    assert_eq!(recorded.label.as_deref(), Some("Hamburg"));
    assert!(recorded.ended_utc.is_some());
    let events = read_events(&path, &id).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[2].stamp().elapsed_ms, 3_600_000);
    assert!(
        matches!(&events[2], Event::Location(LocationEvent { state: LocationState::Fix { source_utc: source, .. }, .. }) if *source == source_utc)
    );
}

#[tokio::test]
async fn writer_errors_are_visible_and_finish_reports_failure() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rides.sqlite3");
    let session = Session::new(None, &Config::default()).unwrap();
    let recorder = Recorder::open(&path, session).await.unwrap();
    let mut errors = recorder.errors();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_recording BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT, 'disk failure injected'); END;").unwrap();
    recorder
        .record(Event::Location(LocationEvent {
            at: Stamp {
                utc: Utc::now(),
                elapsed_ms: 0,
                generation: 0,
            },
            state: LocationState::Denied,
        }))
        .await
        .unwrap();
    errors.changed().await.unwrap();
    assert!(
        errors
            .borrow()
            .as_ref()
            .unwrap()
            .contains("disk failure injected")
    );
    assert!(recorder.finish(Utc::now()).await.is_err());
    assert!(read_sessions(&path).unwrap()[0].ended_utc.is_none());
}

#[tokio::test]
async fn recording_open_failure_is_returned_before_sampling() {
    let directory = tempfile::tempdir().unwrap();
    assert!(
        Recorder::open(
            directory.path(),
            Session::new(None, &Config::default()).unwrap()
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn full_writer_queue_returns_the_event_for_lossless_shutdown_drain() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rides.sqlite3");
    let session = Session::new(None, &Config::default()).unwrap();
    let id = session.id.clone();
    let recorder = Recorder::open(&path, session).await.unwrap();
    let blocker = rusqlite::Connection::open(&path).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut accepted = 0;
    let mut retained = None;
    for elapsed_ms in 0..=STORAGE_QUEUE_CAPACITY + 2 {
        let event = Event::Location(LocationEvent {
            at: Stamp {
                utc: Utc::now(),
                elapsed_ms: elapsed_ms as u64,
                generation: 0,
            },
            state: LocationState::Denied,
        });
        match recorder.try_record(event) {
            Ok(()) => accepted += 1,
            Err(TryRecordError::Full(event)) => {
                retained = Some(*event);
                break;
            }
            Err(error) => panic!("unexpected writer error: {error}"),
        }
    }
    assert!(retained.is_some());
    blocker.execute_batch("ROLLBACK").unwrap();
    recorder.record(retained.unwrap()).await.unwrap();
    recorder.finish(Utc::now()).await.unwrap();
    assert_eq!(read_events(&path, &id).unwrap().len(), accepted + 1);
}
