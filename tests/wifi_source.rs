use network_monitor::wifi::{
    WifiInterface, WifiLink, WifiMode, WifiReader, WifiSnapshot, WifiSource, WifiSourceError,
    WifiWorker,
};
use std::{collections::VecDeque, task::Poll, time::Duration};

struct HiddenSsid;

impl WifiReader for HiddenSsid {
    fn read(&mut self) -> Result<WifiSnapshot, WifiSourceError> {
        Ok(WifiSnapshot {
            interfaces: vec![WifiInterface {
                name: "en0".into(),
                power_reported_on: true,
                mode: WifiMode::Station,
                link: WifiLink::Active,
                ssid: None,
                bssid: None,
            }],
        })
    }
}

#[tokio::test]
async fn association_evidence_survives_an_unavailable_ssid() {
    let source = WifiWorker::new(HiddenSsid).unwrap();
    let observation = source.sample().await.unwrap();

    assert_eq!(observation.interfaces.len(), 1);
    let interface = &observation.interfaces[0];
    assert_eq!(interface.name, "en0");
    assert!(interface.power_reported_on);
    assert_eq!(interface.mode, WifiMode::Station);
    assert_eq!(interface.link, WifiLink::Active);
    assert_eq!(interface.ssid, None);
    assert_eq!(interface.bssid, None);
}

struct Readings(VecDeque<Result<WifiSnapshot, WifiSourceError>>);

impl WifiReader for Readings {
    fn read(&mut self) -> Result<WifiSnapshot, WifiSourceError> {
        self.0.pop_front().unwrap()
    }
}

#[tokio::test]
async fn disconnect_missing_interface_and_query_error_remain_distinct_and_recover() {
    let disconnected = WifiSnapshot {
        interfaces: vec![WifiInterface {
            name: "en0".into(),
            power_reported_on: true,
            mode: WifiMode::None,
            link: WifiLink::Inactive,
            ssid: None,
            bssid: None,
        }],
    };
    let mut recovered = HiddenSsid.read().unwrap();
    recovered.interfaces[0].link = WifiLink::Unavailable {
        reason: "link state key unavailable".into(),
    };
    let source = WifiWorker::new(Readings(VecDeque::from([
        Ok(disconnected.clone()),
        Ok(WifiSnapshot { interfaces: vec![] }),
        Err(WifiSourceError::Native {
            reason: "Wi-Fi subsystem unavailable".into(),
        }),
        Ok(recovered),
    ])))
    .unwrap();

    assert_eq!(source.sample().await.unwrap(), disconnected);
    assert!(source.sample().await.unwrap().interfaces.is_empty());
    assert_eq!(
        source.sample().await.unwrap_err(),
        WifiSourceError::Native {
            reason: "Wi-Fi subsystem unavailable".into()
        }
    );
    let recovered = source.sample().await.unwrap();
    assert_eq!(recovered.interfaces[0].mode, WifiMode::Station);
    assert_eq!(
        recovered.interfaces[0].link,
        WifiLink::Unavailable {
            reason: "link state key unavailable".into()
        }
    );
}

struct ControlledReader {
    started: tokio::sync::mpsc::UnboundedSender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

impl WifiReader for ControlledReader {
    fn read(&mut self) -> Result<WifiSnapshot, WifiSourceError> {
        self.started.send(()).unwrap();
        self.release.recv().unwrap();
        HiddenSsid.read()
    }
}

#[tokio::test]
async fn bounded_worker_skips_cancelled_queued_requests_and_accepts_later_reads() {
    let (started_tx, mut started) = tokio::sync::mpsc::unbounded_channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let source = WifiWorker::new(ControlledReader {
        started: started_tx,
        release: release_rx,
    })
    .unwrap();
    let first = tokio::spawn(source.sample());
    tokio::time::timeout(Duration::from_secs(2), started.recv())
        .await
        .unwrap()
        .unwrap();
    let mut queued = source.sample();
    std::future::poll_fn(|context| {
        assert!(queued.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    assert_eq!(source.sample().await.unwrap_err(), WifiSourceError::Busy);
    drop(queued);
    release.send(()).unwrap();
    first.await.unwrap().unwrap();

    let resumed_source = source.clone();
    let resumed = tokio::spawn(async move {
        loop {
            let result = resumed_source.sample().await;
            if result == Err(WifiSourceError::Busy) {
                tokio::task::yield_now().await;
            } else {
                return result;
            }
        }
    });
    tokio::time::timeout(Duration::from_secs(2), started.recv())
        .await
        .unwrap()
        .unwrap();
    release.send(()).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), resumed)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.interfaces[0].mode, WifiMode::Station);
    drop(source);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), started.recv())
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn cancelling_an_in_flight_sample_does_not_block_the_next_sample() {
    let (started_tx, mut started) = tokio::sync::mpsc::unbounded_channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let source = WifiWorker::new(ControlledReader {
        started: started_tx,
        release: release_rx,
    })
    .unwrap();
    let first = tokio::spawn(source.sample());
    tokio::time::timeout(Duration::from_secs(2), started.recv())
        .await
        .unwrap()
        .unwrap();
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let next = tokio::spawn(source.sample());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), started.recv())
        .await
        .unwrap()
        .unwrap();
    release.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), next)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .interfaces[0]
            .link,
        WifiLink::Active
    );
}
