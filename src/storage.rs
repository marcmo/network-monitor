use std::{
    path::{Path, PathBuf},
    thread,
};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, watch};

use crate::{config::Config, model::Event};

pub const STORAGE_QUEUE_CAPACITY: usize = 128;
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub label: Option<String>,
    pub started_utc: DateTime<Utc>,
    pub ended_utc: Option<DateTime<Utc>>,
    pub app_version: String,
    pub config_json: String,
}

impl Session {
    pub fn new(label: Option<String>, config: &Config) -> Result<Self, StorageError> {
        Ok(Self {
            id: uuid::Uuid::new_v4().to_string(),
            label,
            started_utc: Utc::now(),
            ended_utc: None,
            app_version: env!("CARGO_PKG_VERSION").into(),
            config_json: serde_json::to_string(config)?,
        })
    }
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("SQLite recording failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("recording data is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("recording timestamp is invalid: {0}")]
    Timestamp(#[from] chrono::ParseError),
    #[error("cannot start recording writer: {0}")]
    Io(#[from] std::io::Error),
    #[error("recording writer stopped: {0}")]
    Closed(String),
    #[error("recording integer is outside the SQLite range")]
    IntegerRange,
    #[error("unsupported recording schema version {0}")]
    Schema(i64),
}

enum Command {
    Record(Event),
    Finish(DateTime<Utc>, oneshot::Sender<Result<(), StorageError>>),
}

#[derive(Debug, Error)]
pub enum TryRecordError {
    #[error("recording queue is full; this completed event has not been enqueued")]
    Full(Box<Event>),
    #[error(transparent)]
    Closed(StorageError),
}

pub struct Recorder {
    sender: mpsc::Sender<Command>,
    errors: watch::Receiver<Option<String>>,
    thread: thread::JoinHandle<()>,
}

impl Recorder {
    pub async fn open(path: impl AsRef<Path>, session: Session) -> Result<Self, StorageError> {
        let path = path.as_ref().to_path_buf();
        let (sender, receiver) = mpsc::channel(STORAGE_QUEUE_CAPACITY);
        let (errors_tx, errors) = watch::channel(None);
        let (ready_tx, ready_rx) = oneshot::channel();
        let thread = thread::Builder::new()
            .name("recording".into())
            .spawn(move || writer(path, session, receiver, errors_tx, ready_tx))?;
        ready_rx
            .await
            .map_err(|error| StorageError::Closed(error.to_string()))??;
        Ok(Self {
            sender,
            errors,
            thread,
        })
    }

    pub fn errors(&self) -> watch::Receiver<Option<String>> {
        self.errors.clone()
    }

    pub fn try_record(&self, event: Event) -> Result<(), TryRecordError> {
        self.sender
            .try_send(Command::Record(event))
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(Command::Record(event)) => {
                    TryRecordError::Full(Box::new(event))
                }
                mpsc::error::TrySendError::Full(Command::Finish(_, _)) => {
                    TryRecordError::Closed(StorageError::Closed("unexpected finish command".into()))
                }
                mpsc::error::TrySendError::Closed(_) => TryRecordError::Closed(self.closed_error()),
            })
    }

    pub async fn record(&self, event: Event) -> Result<(), StorageError> {
        self.sender
            .send(Command::Record(event))
            .await
            .map_err(|_| self.closed_error())
    }

    fn closed_error(&self) -> StorageError {
        StorageError::Closed(
            self.errors
                .borrow()
                .clone()
                .unwrap_or_else(|| "channel closed".into()),
        )
    }

    pub async fn finish(self, ended_utc: DateTime<Utc>) -> Result<(), StorageError> {
        let (sender, receiver) = oneshot::channel();
        let result = match self.sender.send(Command::Finish(ended_utc, sender)).await {
            Ok(()) => receiver
                .await
                .map_err(|error| StorageError::Closed(error.to_string()))
                .and_then(|result| result),
            Err(_) => Err(self.closed_error()),
        };
        drop(self.sender);
        let joined = self
            .thread
            .join()
            .map_err(|_| StorageError::Closed("writer panicked".into()));
        result.and(joined)
    }
}

fn open_writer(path: &Path, session: &Session) -> Result<Connection, StorageError> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(std::time::Duration::from_secs(2))?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > i64::from(SCHEMA_VERSION) {
        return Err(StorageError::Schema(version));
    }
    connection.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY, label TEXT, started_utc TEXT NOT NULL, ended_utc TEXT,
            app_version TEXT NOT NULL, config_json TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS events (
            id INTEGER PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id),
            utc TEXT NOT NULL, elapsed_ms INTEGER NOT NULL, generation INTEGER NOT NULL,
            event_type TEXT NOT NULL, payload_json TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS events_session_elapsed ON events(session_id, elapsed_ms);
        PRAGMA user_version=1;",
    )?;
    connection.execute(
        "INSERT INTO sessions(id,label,started_utc,app_version,config_json) VALUES(?1,?2,?3,?4,?5)",
        params![
            session.id,
            session.label,
            session.started_utc.to_rfc3339(),
            session.app_version,
            session.config_json
        ],
    )?;
    Ok(connection)
}

fn writer(
    path: PathBuf,
    session: Session,
    mut receiver: mpsc::Receiver<Command>,
    errors: watch::Sender<Option<String>>,
    ready: oneshot::Sender<Result<(), StorageError>>,
) {
    let connection = match open_writer(&path, &session) {
        Ok(connection) => {
            let _ = ready.send(Ok(()));
            connection
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    while let Some(command) = receiver.blocking_recv() {
        let result = match command {
            Command::Record(event) => insert_event(&connection, &session.id, &event),
            Command::Finish(ended, reply) => {
                let result = connection
                    .execute(
                        "UPDATE sessions SET ended_utc=?1 WHERE id=?2",
                        params![ended.to_rfc3339(), session.id],
                    )
                    .map(|_| ())
                    .map_err(StorageError::from);
                if let Err(error) = &result {
                    let _ = errors.send(Some(error.to_string()));
                }
                let _ = reply.send(result);
                return;
            }
        };
        if let Err(error) = result {
            let _ = errors.send(Some(error.to_string()));
            return;
        }
    }
}

fn insert_event(connection: &Connection, session: &str, event: &Event) -> Result<(), StorageError> {
    let stamp = event.stamp();
    let kind = match event {
        Event::Probe(_) => "probe",
        Event::Traffic(_) => "traffic",
        Event::Gap(_) => "gap",
        Event::Location(_) => "location",
        Event::ClockAdjusted { .. } => "clock_adjusted",
    };
    connection.execute("INSERT INTO events(session_id,utc,elapsed_ms,generation,event_type,payload_json) VALUES(?1,?2,?3,?4,?5,?6)",
        params![session, stamp.utc.to_rfc3339(), i64::try_from(stamp.elapsed_ms).map_err(|_| StorageError::IntegerRange)?, i64::try_from(stamp.generation).map_err(|_| StorageError::IntegerRange)?, kind, serde_json::to_string(event)?])?;
    Ok(())
}

pub fn read_sessions(path: impl AsRef<Path>) -> Result<Vec<Session>, StorageError> {
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement = connection.prepare("SELECT id,label,started_utc,ended_utc,app_version,config_json FROM sessions ORDER BY started_utc,id")?;
    let mut rows = statement.query([])?;
    let mut sessions = Vec::new();
    while let Some(row) = rows.next()? {
        let start: String = row.get(2)?;
        let end: Option<String> = row.get(3)?;
        sessions.push(Session {
            id: row.get(0)?,
            label: row.get(1)?,
            started_utc: start.parse()?,
            ended_utc: end.map(|text| text.parse()).transpose()?,
            app_version: row.get(4)?,
            config_json: row.get(5)?,
        });
    }
    Ok(sessions)
}

pub fn read_events(path: impl AsRef<Path>, session_id: &str) -> Result<Vec<Event>, StorageError> {
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement =
        connection.prepare("SELECT payload_json FROM events WHERE session_id=?1 ORDER BY id")?;
    let mut rows = statement.query([session_id])?;
    let mut events = Vec::new();
    while let Some(row) = rows.next()? {
        let payload: String = row.get(0)?;
        events.push(serde_json::from_str(&payload)?);
    }
    Ok(events)
}
