use chrono::{TimeZone, Utc};
use network_monitor::{
    location::{LocationError, LocationIngress, NativeLocationEvent},
    model::LocationState,
};

#[test]
fn denied_permission_is_an_observation_with_its_receipt_time() {
    let receipt = Utc.timestamp_opt(1_700_000_500, 0).unwrap();
    let event = NativeLocationEvent::Denied.into_raw(receipt);
    assert_eq!(event.state, LocationState::Denied);
    assert_eq!(event.received_utc, receipt);
}

#[test]
fn old_inaccurate_fix_preserves_source_time_and_accuracy_without_inventing_freshness() {
    let receipt = Utc.timestamp_opt(1_700_000_500, 0).unwrap();
    let event = NativeLocationEvent::Fix {
        latitude: 48.1,
        longitude: 11.5,
        horizontal_accuracy_m: 2_000.0,
        source_unix_seconds: 1_700_000_000.25,
    }
    .into_raw(receipt);
    assert_eq!(event.received_utc, receipt);
    assert_eq!(
        event.state,
        LocationState::Fix {
            latitude: 48.1,
            longitude: 11.5,
            horizontal_accuracy_m: 2_000.0,
            source_utc: Utc.timestamp_opt(1_700_000_000, 250_000_000).unwrap(),
        }
    );
}

#[test]
fn missing_location_preserves_the_provider_failure_reason() {
    let receipt = Utc.timestamp_opt(1_700_000_500, 0).unwrap();
    let event = NativeLocationEvent::Unavailable {
        reason: "Location is currently unknown".into(),
    }
    .into_raw(receipt);
    assert_eq!(event.received_utc, receipt);
    assert_eq!(
        event.state,
        LocationState::Unavailable {
            reason: "Location is currently unknown".into()
        }
    );
}

#[test]
fn invalid_fixes_are_explicitly_unavailable() {
    let receipt = Utc.timestamp_opt(1_700_000_500, 0).unwrap();
    for (latitude, longitude, accuracy, time) in [
        (91.0, 11.5, 10.0, 1_700_000_000.0),
        (48.1, -181.0, 10.0, 1_700_000_000.0),
        (f64::NAN, 11.5, 10.0, 1_700_000_000.0),
        (48.1, f64::INFINITY, 10.0, 1_700_000_000.0),
        (48.1, 11.5, -1.0, 1_700_000_000.0),
        (48.1, 11.5, f64::NAN, 1_700_000_000.0),
        (48.1, 11.5, 10.0, f64::NAN),
        (48.1, 11.5, 10.0, f64::INFINITY),
        (48.1, 11.5, 10.0, f64::MAX),
    ] {
        let event = NativeLocationEvent::Fix {
            latitude,
            longitude,
            horizontal_accuracy_m: accuracy,
            source_unix_seconds: time,
        }
        .into_raw(receipt);
        assert!(matches!(event.state, LocationState::Unavailable { .. }));
        assert_eq!(event.received_utc, receipt);
    }
}

#[test]
fn full_location_ingress_fails_explicitly_without_replacing_an_earlier_fix() {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let ingress = LocationIngress::new(sender);
    let receipt = Utc.timestamp_opt(1_700_000_500, 0).unwrap();
    ingress
        .record(NativeLocationEvent::Pending, receipt)
        .unwrap();
    assert_eq!(
        ingress.record(NativeLocationEvent::Denied, receipt),
        Err(LocationError::Backpressure)
    );
    assert_eq!(receiver.try_recv().unwrap().state, LocationState::Pending);
}

#[test]
fn closed_receiver_ends_location_ingress_without_blocking() {
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    drop(receiver);
    let ingress = LocationIngress::new(sender);
    let receipt = Utc.timestamp_opt(1_700_000_500, 0).unwrap();
    assert_eq!(
        ingress.record(NativeLocationEvent::Pending, receipt),
        Err(LocationError::ReceiverClosed)
    );
}

#[test]
#[cfg(target_os = "macos")]
fn location_capture_refuses_a_worker_thread_before_requesting_permission() {
    let (sender, _receiver) = tokio::sync::mpsc::channel(1);
    let (stop, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
    assert_eq!(
        network_monitor::location::run_main(sender, stop),
        Err(LocationError::WrongThread)
    );
}
