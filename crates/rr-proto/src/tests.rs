//! Unit tests for the `RemoteRadio` wire protocol.
//!
//! These assert (a) round-trip stability, (b) the *on-wire tag strings* so an
//! accidental rename of an enum variant that would silently break
//! interoperability fails the build, (c) version negotiation, and (d) graceful
//! rejection of malformed input.

#![allow(clippy::unwrap_used, reason = "unwrap is acceptable in test code")]
#![allow(clippy::expect_used, reason = "expect is acceptable in test code")]

use super::*;

// -- version negotiation ----------------------------------------------------

#[test]
fn version_matches_itself() {
    assert!(check_version(PROTOCOL_VERSION).is_ok());
}

#[test]
fn version_mismatch_is_rejected() {
    let err = check_version(PROTOCOL_VERSION + 1).unwrap_err();
    match err {
        ProtoError::IncompatibleVersion { local, remote } => {
            assert_eq!(local, PROTOCOL_VERSION);
            assert_eq!(remote, PROTOCOL_VERSION + 1);
        }
        other => panic!("expected IncompatibleVersion, got {other:?}"),
    }
}

// -- presence ---------------------------------------------------------------

#[test]
fn presence_round_trips() {
    let p = Presence {
        version: PROTOCOL_VERSION,
        role: Role::Station,
        monitors: vec![MonitorInfo {
            id: 0,
            label: "Monitor 1 (2560x1440)".into(),
            width: 2560,
            height: 1440,
            primary: true,
        }],
        controller: Some(42),
        caps: Caps {
            clipboard: true,
            file_transfer: true,
            codecs: vec!["h264".into(), "vp8".into()],
        },
    };
    let json = p.to_json().expect("serialise");
    let back = Presence::from_json(&json).expect("deserialise");
    assert_eq!(p, back);
}

#[test]
fn presence_role_tag_is_stable() {
    let json = Presence {
        version: 1,
        role: Role::Station,
        monitors: vec![],
        controller: None,
        caps: Caps::default(),
    }
    .to_json()
    .unwrap();
    // Role must serialise lowercase; a rename would break cross-version peers.
    assert!(json.contains("\"role\":\"station\""), "got {json}");
}

#[test]
fn presence_tolerates_missing_optional_fields() {
    // A minimal Station announcement from a future/compact peer.
    let json = r#"{"version":1,"role":"client"}"#;
    let p = Presence::from_json(json).expect("deserialise minimal");
    assert_eq!(p.role, Role::Client);
    assert!(p.monitors.is_empty());
    assert_eq!(p.controller, None);
    assert!(!p.caps.clipboard);
}

// -- control channel --------------------------------------------------------

#[test]
fn control_input_round_trips() {
    let msg = ControlMsg::Input(InputEvent::MouseMove {
        monitor: 1,
        x: 100,
        y: 250,
    });
    let bytes = msg.encode().unwrap();
    assert_eq!(ControlMsg::decode(&bytes).unwrap(), msg);
}

#[test]
fn control_all_variants_round_trip() {
    let cases = vec![
        ControlMsg::Input(InputEvent::MouseButton {
            button: MouseButton::Right,
            down: true,
        }),
        ControlMsg::Input(InputEvent::Wheel { dx: 0, dy: -3 }),
        ControlMsg::Input(InputEvent::Key {
            code: 65,
            down: false,
        }),
        ControlMsg::Input(InputEvent::Text {
            text: "hello".into(),
        }),
        ControlMsg::RequestControl,
        ControlMsg::ApproveHandoff { to: 7 },
        ControlMsg::DenyHandoff { to: 7 },
        ControlMsg::ReleaseControl,
        ControlMsg::ControlState {
            controller: Some(3),
            pending: None,
        },
        ControlMsg::SelectMonitor { monitor: 2 },
        ControlMsg::RequestKeyframe,
    ];
    for c in cases {
        let bytes = c.encode().unwrap();
        let back = ControlMsg::decode(&bytes).unwrap();
        assert_eq!(c, back, "round-trip mismatch for {c:?}");
    }
}

#[test]
fn control_tags_are_stable() {
    let json = String::from_utf8(ControlMsg::RequestControl.encode().unwrap()).unwrap();
    assert!(json.contains("\"t\":\"request_control\""), "got {json}");

    let json = String::from_utf8(
        ControlMsg::Input(InputEvent::MouseMove {
            monitor: 0,
            x: 1,
            y: 2,
        })
        .encode()
        .unwrap(),
    )
    .unwrap();
    assert!(json.contains("\"t\":\"input\""), "got {json}");
    assert!(json.contains("\"ev\":\"mouse_move\""), "got {json}");
}

#[test]
fn control_decode_rejects_garbage() {
    assert!(ControlMsg::decode(b"not json").is_err());
    assert!(ControlMsg::decode(b"{\"t\":\"no_such_variant\"}").is_err());
}

// -- clipboard --------------------------------------------------------------

#[test]
fn clipboard_round_trips() {
    let m = ClipboardMsg::Text {
        text: "copied".into(),
    };
    let bytes = serde_json::to_vec(&m).unwrap();
    let back: ClipboardMsg = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(m, back);
}

// -- files ------------------------------------------------------------------

#[test]
fn file_offer_round_trips() {
    let m = FileMsg::Offer {
        id: 1,
        name: "report.pdf".into(),
        size: 1024,
        sha256: "abc123".into(),
    };
    let bytes = serde_json::to_vec(&m).unwrap();
    let back: FileMsg = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(m, back);
}

#[test]
fn file_flow_variants_round_trip() {
    let cases = vec![
        FileMsg::Accept { id: 1 },
        FileMsg::Reject {
            id: 1,
            reason: "too big".into(),
        },
        FileMsg::Chunk {
            id: 1,
            seq: 0,
            data_base64: "AAAA".into(),
        },
        FileMsg::Complete { id: 1 },
    ];
    for c in cases {
        let bytes = serde_json::to_vec(&c).unwrap();
        let back: FileMsg = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(c, back);
    }
}
