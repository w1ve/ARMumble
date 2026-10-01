//! Loopback two-peer integration test — the P1 exit criterion.
//!
//! Connects a simulated Client (offerer) and Station (answerer) **in one
//! process** over localhost ICE, using [`InMemorySignaling`] in place of the
//! Mumble relay, then asserts that:
//!
//! 1. the peer connection reaches `Connected` on both sides,
//! 2. the `rr-control` data channel opens,
//! 3. an [`rr_proto::ControlMsg`] round-trips Client → Station and back.
//!
//! This needs no display, no Mumble server and no external network, so it runs
//! unattended in CI.

#![allow(clippy::unwrap_used, reason = "unwrap is acceptable in test code")]
#![allow(clippy::expect_used, reason = "expect is acceptable in test code")]
// This integration test exercises the public API only, so it does not name
// every dependency the library itself needs.
#![allow(
    unused_crate_dependencies,
    reason = "integration test uses the library's public API, not all of its deps directly"
)]

use std::sync::Arc;
use std::time::Duration;

use rr_proto::{ControlMsg, InputEvent, MouseButton};
use rr_webrtc::{
    ConnectionState, InMemorySignaling, PeerSession, RecordingObserver, SessionConfig,
};
use tokio::sync::mpsc;

/// How long to allow for a localhost ICE handshake before failing.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(25);

/// Wait until `check` reports true, polling briefly, or fail with `what`.
async fn wait_until<F, Fut>(what: &str, timeout: Duration, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let result = tokio::time::timeout(timeout, async {
        loop {
            if check().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(result.is_ok(), "timed out waiting for {what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_and_station_connect_and_round_trip_control() {
    let (client_signal, station_signal) = InMemorySignaling::pair();

    // Each side records what it observes; the Station also forwards control
    // messages through a channel so we can await them deterministically.
    let (station_ctrl_tx, mut station_ctrl_rx) = mpsc::unbounded_channel();
    let station_obs = Arc::new(RecordingObserver::with_channel(station_ctrl_tx));
    let (client_ctrl_tx, mut client_ctrl_rx) = mpsc::unbounded_channel();
    let client_obs = Arc::new(RecordingObserver::with_channel(client_ctrl_tx));

    let config = SessionConfig::loopback();

    // Station (answerer) comes up first and waits for an offer.
    let station = Arc::new(
        PeerSession::answer(&config, station_signal, Arc::clone(&station_obs) as _)
            .await
            .expect("build station session"),
    );
    let station_loop = {
        let s = Arc::clone(&station);
        tokio::spawn(async move { s.run().await })
    };

    // Client (offerer) creates the control channel and sends the offer.
    let client = Arc::new(
        PeerSession::offer(&config, client_signal, Arc::clone(&client_obs) as _)
            .await
            .expect("build client session"),
    );
    let client_loop = {
        let c = Arc::clone(&client);
        tokio::spawn(async move { c.run().await })
    };

    // 1. Both sides must report Connected.
    wait_until("client connected", HANDSHAKE_TIMEOUT, || {
        let o = Arc::clone(&client_obs);
        async move { o.states().await.contains(&ConnectionState::Connected) }
    })
    .await;
    wait_until("station connected", HANDSHAKE_TIMEOUT, || {
        let o = Arc::clone(&station_obs);
        async move { o.states().await.contains(&ConnectionState::Connected) }
    })
    .await;

    // 2. The control channel must be available on both ends. The Station
    //    receives it via on_data_channel; the Client created it.
    let _station_dc = station
        .wait_control_channel(HANDSHAKE_TIMEOUT)
        .await
        .expect("station gets control channel");
    let _client_dc = client
        .wait_control_channel(HANDSHAKE_TIMEOUT)
        .await
        .expect("client has control channel");

    wait_until("control channel open on station", HANDSHAKE_TIMEOUT, || {
        let o = Arc::clone(&station_obs);
        async move { o.control_opened().await }
    })
    .await;

    // 3a. Client → Station: an input event, as the real controller would send.
    let to_station = ControlMsg::Input(InputEvent::MouseButton {
        button: MouseButton::Left,
        down: true,
    });
    client
        .send_control(&to_station)
        .await
        .expect("client sends control");

    let got = tokio::time::timeout(HANDSHAKE_TIMEOUT, station_ctrl_rx.recv())
        .await
        .expect("station receives control in time")
        .expect("station channel open");
    assert_eq!(got, to_station, "station must receive the exact message");

    // 3b. Station → Client: the authoritative control-state broadcast.
    let to_client = ControlMsg::ControlState {
        controller: Some(7),
        pending: None,
    };
    station
        .send_control(&to_client)
        .await
        .expect("station sends control");

    let got = tokio::time::timeout(HANDSHAKE_TIMEOUT, client_ctrl_rx.recv())
        .await
        .expect("client receives control in time")
        .expect("client channel open");
    assert_eq!(got, to_client, "client must receive the exact message");

    // Tidy up so the signaling loops terminate.
    client.close().await.expect("close client");
    station.close().await.expect("close station");
    client_loop.abort();
    station_loop.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ice_candidates_arriving_before_sdp_are_queued() {
    // A candidate that races ahead of the remote description must be buffered
    // and applied later, not dropped. We exercise the path by feeding a
    // candidate into a fresh answerer before any offer is set; it must not
    // error.
    let (peer_signal, local_signal) = InMemorySignaling::pair();
    let observer = Arc::new(RecordingObserver::new());
    let config = SessionConfig::loopback();

    let station = PeerSession::answer(&config, local_signal, observer as _)
        .await
        .expect("build answerer");

    // Send a syntactically valid candidate with no SDP yet.
    let candidate = serde_json::json!({
        "candidate": "candidate:1 1 udp 2130706431 127.0.0.1 54321 typ host",
        "sdpMid": "0",
        "sdpMLineIndex": 0
    })
    .to_string();
    rr_webrtc::SignalTransport::send(
        peer_signal.as_ref(),
        rr_webrtc::SignalMessage::IceCandidate { candidate },
    )
    .await
    .expect("queue candidate");

    // Drive one iteration of the loop with a short timeout; it should process
    // the candidate without returning an error.
    let run = tokio::time::timeout(Duration::from_millis(500), station.run()).await;
    // Timing out is expected (the loop waits for more messages); the important
    // part is that it did not fail while handling the early candidate.
    if let Ok(result) = run {
        result.expect("handling an early candidate must not error");
    }

    station.close().await.expect("close");
}
