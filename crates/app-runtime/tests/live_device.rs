//! Checks the playlist prediction path against a real Linn DS.
//!
//! Ignored by default and skipped without an address, because CI has no
//! streamer. The wire formats in `app_runtime::playlist` were captured this
//! way, and this is how to re-check them when a firmware or DS update lands:
//!
//! ```sh
//! LINN_TEST_HOST=192.168.7.218 cargo test -p app-runtime --features std \
//!     --test live_device -- --ignored --nocapture
//! ```

use app_runtime::{
    host_tcp::HostTcpConnector, lpec::LpecSession, net::Endpoint, net::TcpConnector, playlist::Step,
};

fn endpoint_from_env() -> Option<Endpoint> {
    let host = std::env::var("LINN_TEST_HOST").ok()?;
    let mut octets = [0_u8; 4];
    let mut parts = host.split('.');
    for octet in &mut octets {
        *octet = parts.next()?.parse().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    let port = std::env::var("LINN_TEST_PORT")
        .ok()
        .and_then(|port| port.parse().ok())
        .unwrap_or(23);
    Some(Endpoint::ipv4(octets, port))
}

#[test]
#[ignore]
fn predicts_the_next_track_from_a_real_device() {
    let Some(endpoint) = endpoint_from_env() else {
        eprintln!("set LINN_TEST_HOST to run this");
        return;
    };

    let mut connector = HostTcpConnector::new();
    let mut session = LpecSession::new();

    // First poll subscribes; the second drains the initial state burst.
    for tick in 0..2 {
        let mut stream = connector.connect_events(endpoint).unwrap();
        let _ = session.poll(&mut stream, tick * 100);
    }

    let queue_len = session.playlist().len();
    let current = session.playlist().current_id();
    let next = session.playlist().neighbour_id(Step::Forward);
    println!("queue={queue_len} current={current:?} next={next:?}");
    assert!(queue_len > 0, "expected a queue from a playing device");

    {
        // Prefetch runs on the command connection: the event connection's read
        // timeout is far too short to wait on an action.
        let mut stream = connector.connect(endpoint).unwrap();
        session.prefetch_neighbours(&mut stream).unwrap();
    }

    let Some(predicted) = session.predict_skip(Step::Forward, 200) else {
        // The end of a queue without repeat is a legitimate no-prediction.
        assert!(next.is_none(), "had a next track but predicted nothing");
        return;
    };
    println!(
        "predicted {:?} / {:?} / {}s / artwork {}",
        predicted.title.as_str(),
        predicted.artist.as_str(),
        predicted.duration_seconds,
        !predicted.album_art_uri.is_empty()
    );
    assert!(!predicted.title.is_empty(), "prediction had no title");
}
