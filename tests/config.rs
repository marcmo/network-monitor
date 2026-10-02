use network_monitor::config::Config;

#[test]
fn defaults_fit_the_detection_budget_and_reject_unbounded_or_nonindependent_checks() {
    let defaults = Config::default();
    defaults.validate().unwrap();
    assert!(defaults.tcp_interval_ms + defaults.tcp_timeout_ms + 1000 <= 5000);
    let mut same_destination = defaults.clone();
    same_destination.tcp_targets = vec!["127.0.0.1:443".into(), "127.0.0.1:444".into()];
    assert!(same_destination.validate().is_err());
    let mut unbounded = defaults.clone();
    unbounded.tcp_timeout_ms = 3000;
    assert!(unbounded.validate().is_err());
    let mut plaintext = defaults;
    plaintext.https_url = "http://example.com".into();
    assert!(plaintext.validate().is_err());
}

#[test]
fn configuration_round_trip_preserves_quality_thresholds_and_rejects_unknown_fields() {
    let config = Config {
        slow_latency_ms: 400,
        ..Config::default()
    };
    let serialized = toml::to_string_pretty(&config).unwrap();
    let decoded: Config = toml::from_str(&serialized).unwrap();
    assert_eq!(decoded.slow_latency_ms, 400);
    assert!(toml::from_str::<Config>("automatic_speed_test = true").is_err());
}

#[test]
fn alternate_ipv6_spellings_and_ipv4_mapped_addresses_are_one_destination() {
    for targets in [
        ["[::1]:443", "[0:0:0:0:0:0:0:1]:444"],
        ["127.0.0.1:443", "[::ffff:127.0.0.1]:444"],
    ] {
        let config = Config {
            tcp_targets: targets.map(str::to_owned).to_vec(),
            ..Config::default()
        };
        assert!(config.validate().is_err());
    }
}
