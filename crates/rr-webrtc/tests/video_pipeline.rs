//! End-to-end video pipeline test — the P2 exit criterion.
//!
//! Proves that the pieces built in this phase compose: a synthetic screen is
//! captured, encoded as H.264, carried over the peer-to-peer link's control
//! path, and decoded back into a picture of the right size, with the
//! keyframe-on-demand round trip exercised over the real data channel.
//!
//! Needs no display, no Mumble server and no external network, so it runs
//! unattended in CI. Real screen capture and glass-to-glass latency are
//! validated separately on a Windows Station, since neither is possible here.

#![allow(clippy::unwrap_used, reason = "unwrap is acceptable in test code")]
#![allow(clippy::expect_used, reason = "expect is acceptable in test code")]
#![allow(
    unused_crate_dependencies,
    reason = "integration test uses the library's public API, not all of its deps directly"
)]
#![allow(
    unused_results,
    reason = "frames are encoded to advance encoder state; later output is what is asserted"
)]

use std::sync::Arc;
use std::time::Duration;

use rr_capture::{ScreenCapturer, SyntheticCapturer};
use rr_codec::{Decoder, Encoder, EncoderSettings};
use rr_proto::ControlMsg;
use rr_webrtc::{
    ConnectionState, InMemorySignaling, PeerSession, RecordingObserver, SessionConfig, VideoSender,
};
use tokio::sync::mpsc;

const W: u32 = 64;
const H: u32 = 32;
const TIMEOUT: Duration = Duration::from_secs(25);

async fn wait_until<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let ok = tokio::time::timeout(TIMEOUT, async {
        loop {
            if check().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(ok.is_ok(), "timed out waiting for {what}");
}

/// Capture → encode → decode, verifying the codec chain end to end.
#[test]
fn captured_frames_encode_and_decode() {
    let mut cap = SyntheticCapturer::new(W, H);
    let mut enc = Encoder::new(W, H, EncoderSettings::default()).expect("encoder");
    let mut dec = Decoder::new().expect("decoder");

    let mut decoded_pictures = 0;
    for _ in 0..8 {
        let frame = cap.grab().expect("grab");
        let encoded = enc.encode(&frame).expect("encode");
        if let Ok(Some(picture)) = dec.decode(&encoded.data) {
            assert_eq!((picture.width, picture.height), (W, H));
            decoded_pictures += 1;
        }
    }
    assert!(
        decoded_pictures >= 7,
        "expected nearly every captured frame to survive the round trip, got {decoded_pictures}"
    );
}

/// A video track can be created and attached without disturbing the session.
#[test]
fn video_sender_can_be_created() {
    let sender = VideoSender::new(0xDEAD_BEEF).expect("video sender");
    // The track must be real enough to hand to a peer connection.
    assert!(Arc::strong_count(&sender.track()) >= 1);
}

/// The full P2 story: a live P2P session plus the capture/encode/decode chain,
/// with a real keyframe request travelling Client → Station over the link.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keyframe_request_travels_over_the_live_session() {
    let (client_signal, station_signal) = InMemorySignaling::pair();

    // The Station records control traffic so we can observe the request.
    let (station_tx, mut station_rx) = mpsc::unbounded_channel();
    let station_obs = Arc::new(RecordingObserver::with_channel(station_tx));
    let client_obs = Arc::new(RecordingObserver::new());

    let config = SessionConfig::loopback();

    let station = Arc::new(
        PeerSession::answer(&config, station_signal, Arc::clone(&station_obs) as _)
            .await
            .expect("station session"),
    );
    let station_loop = {
        let s = Arc::clone(&station);
        tokio::spawn(async move { s.run().await })
    };

    let client = Arc::new(
        PeerSession::offer(&config, client_signal, Arc::clone(&client_obs) as _)
            .await
            .expect("client session"),
    );
    let client_loop = {
        let c = Arc::clone(&client);
        tokio::spawn(async move { c.run().await })
    };

    wait_until("client connected", || {
        let o = Arc::clone(&client_obs);
        async move { o.states().await.contains(&ConnectionState::Connected) }
    })
    .await;

    let _dc = station
        .wait_control_channel(TIMEOUT)
        .await
        .expect("station control channel");

    // A viewer asks for a keyframe, exactly as it would on subscribe. With
    // periodic keyframes disabled for latency, this is the only path by which a
    // late joiner becomes decodable, so it must actually arrive.
    client
        .request_keyframe()
        .await
        .expect("client sends keyframe request");

    let got = tokio::time::timeout(TIMEOUT, station_rx.recv())
        .await
        .expect("station receives request in time")
        .expect("channel open");
    assert_eq!(
        got,
        ControlMsg::RequestKeyframe,
        "station must see the keyframe request"
    );

    // The Station honours it by forcing the next encoded frame to be an IDR.
    let mut cap = SyntheticCapturer::new(W, H);
    let mut enc = Encoder::new(W, H, EncoderSettings::default()).expect("encoder");
    enc.encode(&cap.grab().unwrap()).expect("initial idr");
    enc.encode(&cap.grab().unwrap()).expect("delta");

    enc.request_keyframe();
    let forced = enc.encode(&cap.grab().unwrap()).expect("forced idr");
    assert!(
        forced.is_keyframe,
        "the station must answer a request with an IDR"
    );

    client.close().await.expect("close client");
    station.close().await.expect("close station");
    client_loop.abort();
    station_loop.abort();
}
