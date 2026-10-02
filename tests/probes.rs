use network_monitor::{
    config::Config,
    model::*,
    probe::NetworkProbes,
    scheduler::{ProbeRunner, ProbeSpec},
};

#[tokio::test]
async fn tcp_probe_distinguishes_accepted_and_refused_connections() {
    let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = server.local_addr().unwrap().to_string();
    let probes = NetworkProbes::new(&Config::default()).unwrap();
    let spec = ProbeSpec {
        kind: ProbeKind::Tcp,
        target: address,
        interval_ms: 2000,
        timeout_ms: 1500,
    };
    assert_eq!(probes.probe(spec.clone()).await, ProbeOutcome::Success);
    drop(server);
    assert_eq!(probes.probe(spec).await, ProbeOutcome::Refused);
}

#[tokio::test]
async fn resolver_accepts_both_normal_simultaneous_callers_without_local_capacity_failures() {
    let probes = NetworkProbes::new(&Config::default()).unwrap();
    let spec = ProbeSpec {
        kind: ProbeKind::Dns,
        target: "localhost".into(),
        interval_ms: 15000,
        timeout_ms: 2000,
    };
    for _ in 0..100 {
        let (dns, https_resolution) =
            tokio::join!(probes.probe(spec.clone()), probes.probe(spec.clone()));
        assert_eq!(dns, ProbeOutcome::Success);
        assert_eq!(https_resolution, ProbeOutcome::Success);
    }
}
