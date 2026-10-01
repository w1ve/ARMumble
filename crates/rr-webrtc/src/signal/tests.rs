//! Unit tests for the signaling abstraction.

#![allow(clippy::unwrap_used, reason = "unwrap is acceptable in test code")]
#![allow(clippy::expect_used, reason = "expect is acceptable in test code")]

use super::*;

#[tokio::test]
async fn in_memory_pair_delivers_both_directions() {
    let (a, b) = InMemorySignaling::pair();

    a.send(SignalMessage::Offer { sdp: "o=A".into() })
        .await
        .expect("send offer");
    let got = b.recv().await.expect("b receives");
    assert_eq!(got, SignalMessage::Offer { sdp: "o=A".into() });

    b.send(SignalMessage::Answer { sdp: "a=B".into() })
        .await
        .expect("send answer");
    let got = a.recv().await.expect("a receives");
    assert_eq!(got, SignalMessage::Answer { sdp: "a=B".into() });
}

#[tokio::test]
async fn messages_preserve_order() {
    let (a, b) = InMemorySignaling::pair();
    for i in 0..5 {
        a.send(SignalMessage::IceCandidate {
            candidate: format!("cand-{i}"),
        })
        .await
        .expect("send");
    }
    for i in 0..5 {
        let got = b.recv().await.expect("recv");
        assert_eq!(
            got,
            SignalMessage::IceCandidate {
                candidate: format!("cand-{i}")
            }
        );
    }
}

#[tokio::test]
async fn recv_returns_none_after_peer_dropped() {
    let (a, b) = InMemorySignaling::pair();
    drop(a);
    assert!(b.recv().await.is_none(), "closed channel must yield None");
}

#[test]
fn signal_message_tags_are_stable() {
    // The wire tags map onto Mumble's WebRtcSignal SignalType values; renaming
    // a variant would silently break interoperability.
    let cases = vec![
        (SignalMessage::Stop, "\"kind\":\"stop\""),
        (
            SignalMessage::Offer { sdp: String::new() },
            "\"kind\":\"offer\"",
        ),
        (
            SignalMessage::Answer { sdp: String::new() },
            "\"kind\":\"answer\"",
        ),
        (
            SignalMessage::IceCandidate {
                candidate: String::new(),
            },
            "\"kind\":\"ice_candidate\"",
        ),
        (
            SignalMessage::Start {
                payload: String::new(),
            },
            "\"kind\":\"start\"",
        ),
    ];
    for (msg, expected) in cases {
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(expected), "got {json}");
    }
}

#[test]
fn signal_message_round_trips() {
    let msg = SignalMessage::IceCandidate {
        candidate: "{\"candidate\":\"x\"}".into(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: SignalMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, back);
}
