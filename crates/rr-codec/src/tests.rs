//! Codec unit tests.
//!
//! These run a real `OpenH264` encode/decode round trip on synthetic frames, so
//! they need no display and no network and are safe for CI.

#![allow(clippy::unwrap_used, reason = "unwrap is acceptable in test code")]
#![allow(clippy::expect_used, reason = "expect is acceptable in test code")]
#![allow(
    unused_results,
    reason = "tests encode frames purely to advance encoder state and assert on later output"
)]

use rr_capture::{ScreenCapturer, SyntheticCapturer};

use super::*;

/// Small but legal frame size: even dimensions, big enough to be meaningful.
const W: u32 = 64;
const H: u32 = 32;

fn encoder() -> Encoder {
    Encoder::new(W, H, EncoderSettings::default()).expect("create encoder")
}

#[test]
fn geometry_rejects_zero_and_odd_dimensions() {
    assert!(check_geometry(0, 16).is_err(), "zero width must fail");
    assert!(check_geometry(16, 0).is_err(), "zero height must fail");
    assert!(check_geometry(15, 16).is_err(), "odd width must fail");
    assert!(check_geometry(16, 15).is_err(), "odd height must fail");
    check_geometry(16, 16).expect("even non-zero dimensions are valid");
}

#[test]
fn encoder_rejects_odd_dimensions() {
    let err = Encoder::new(65, 32, EncoderSettings::default()).unwrap_err();
    assert!(matches!(err, CodecError::Geometry { .. }));
}

#[test]
fn encoder_reports_its_dimensions() {
    assert_eq!(encoder().dimensions(), (W, H));
}

#[test]
fn first_frame_is_always_a_keyframe() {
    // A stream must open with an IDR, otherwise the first viewer can never
    // decode anything.
    let mut enc = encoder();
    assert!(enc.keyframe_pending(), "a fresh encoder owes a keyframe");

    let mut cap = SyntheticCapturer::new(W, H);
    let frame = cap.grab().unwrap();
    let encoded = enc.encode(&frame).expect("encode first frame");
    assert!(encoded.is_keyframe, "first frame must be an IDR");
    assert!(!encoded.data.is_empty(), "IDR must carry data");
    assert!(!enc.keyframe_pending(), "request satisfied after encoding");
}

#[test]
fn subsequent_frames_are_not_keyframes() {
    // With periodic intra frames disabled, only requested frames are IDRs.
    let mut enc = encoder();
    let mut cap = SyntheticCapturer::new(W, H);

    let first = enc.encode(&cap.grab().unwrap()).unwrap();
    assert!(first.is_keyframe);

    for _ in 0..4 {
        let e = enc.encode(&cap.grab().unwrap()).unwrap();
        assert!(!e.is_keyframe, "no unsolicited keyframes");
    }
}

#[test]
fn request_keyframe_forces_the_next_frame_to_be_idr() {
    let mut enc = encoder();
    let mut cap = SyntheticCapturer::new(W, H);

    enc.encode(&cap.grab().unwrap()).unwrap(); // initial IDR
    enc.encode(&cap.grab().unwrap()).unwrap(); // delta

    enc.request_keyframe();
    assert!(enc.keyframe_pending());
    let forced = enc.encode(&cap.grab().unwrap()).unwrap();
    assert!(forced.is_keyframe, "requested keyframe must be honoured");

    // And the request is one-shot, not sticky.
    let after = enc.encode(&cap.grab().unwrap()).unwrap();
    assert!(!after.is_keyframe);
}

#[test]
fn encoder_rejects_mismatched_frame_size() {
    let mut enc = encoder();
    let mut cap = SyntheticCapturer::new(W * 2, H);
    let err = enc.encode(&cap.grab().unwrap()).unwrap_err();
    assert!(matches!(err, CodecError::Geometry { .. }));
}

#[test]
fn sequence_number_is_preserved_through_encoding() {
    // The pipeline correlates capture timing with encoded output by sequence.
    let mut enc = encoder();
    let mut cap = SyntheticCapturer::new(W, H);
    for expected in 0..3u64 {
        let frame = cap.grab().unwrap();
        assert_eq!(frame.sequence, expected);
        let encoded = enc.encode(&frame).unwrap();
        assert_eq!(encoded.sequence, expected);
    }
}

#[test]
fn padded_frames_encode_correctly() {
    // Rows with padding must be repacked; otherwise the image would shear.
    let mut enc = encoder();
    let mut cap = SyntheticCapturer::new(W, H).with_row_padding(32);
    let frame = cap.grab().unwrap();
    assert_ne!(frame.stride, W as usize * 4, "test needs real padding");
    let encoded = enc.encode(&frame).expect("padded frame must encode");
    assert!(!encoded.data.is_empty());
}

#[test]
fn encode_then_decode_round_trip() {
    // The core guarantee: a frame encoded by the Station decodes on the Client
    // at the right size.
    let mut enc = encoder();
    let mut dec = Decoder::new().expect("create decoder");
    let mut cap = SyntheticCapturer::new(W, H);

    let encoded = enc.encode(&cap.grab().unwrap()).expect("encode");
    let decoded = dec
        .decode(&encoded.data)
        .expect("decode must not error")
        .expect("IDR must yield a picture");

    assert_eq!(decoded.width, W);
    assert_eq!(decoded.height, H);
    assert_eq!(decoded.rgba.len(), (W * H * 4) as usize);
    assert_eq!(decoded.stride(), W as usize * 4);
}

#[test]
fn decoded_frame_resembles_the_source() {
    // H.264 is lossy, so assert approximate similarity rather than equality.
    // This catches gross errors (channel swaps, shearing, wrong geometry) that
    // a "did it decode" check would miss.
    let mut enc = encoder();
    let mut dec = Decoder::new().unwrap();
    let mut cap = SyntheticCapturer::new(W, H);

    let source = cap.grab().unwrap();
    let encoded = enc.encode(&source).unwrap();
    let decoded = dec.decode(&encoded.data).unwrap().expect("picture");

    // Compare the flat gradient background away from the moving bar.
    let mut compared = 0u32;
    let mut total_delta = 0u32;
    for y in (2..H).step_by(7) {
        for x in (4..W).step_by(5) {
            let expect = SyntheticCapturer::expected_pixel(W, x, y, source.sequence);
            // Skip the white bar column, where ringing is strongest.
            if expect == [255, 255, 255, 255] {
                continue;
            }
            let got = decoded.pixel(x, y).expect("pixel in range");
            for ch in 0..3 {
                total_delta += u32::from(expect[ch].abs_diff(got[ch]));
                compared += 1;
            }
        }
    }

    assert!(compared > 0, "test must compare some pixels");
    let mean_delta = total_delta / compared;
    assert!(
        mean_delta < 40,
        "decoded image differs too much from source (mean channel delta {mean_delta})"
    );
}

#[test]
fn decoder_tolerates_garbage_without_panicking() {
    // A viewer joining mid-stream, or packet loss, will feed the decoder
    // undecodable bytes. That must surface as Ok(None)/Err, never a panic.
    let mut dec = Decoder::new().unwrap();
    let result = dec.decode(&[0x00, 0x00, 0x00, 0x01, 0xff, 0xee, 0xdd]);
    match result {
        Ok(_) | Err(CodecError::Decode(_)) => {}
        Err(other) => panic!("unexpected error kind: {other:?}"),
    }
}

#[test]
fn multiple_frames_decode_in_sequence() {
    // Delta frames must decode against their reference, not just the IDR.
    let mut enc = encoder();
    let mut dec = Decoder::new().unwrap();
    let mut cap = SyntheticCapturer::new(W, H);

    let mut pictures = 0;
    for _ in 0..6 {
        let encoded = enc.encode(&cap.grab().unwrap()).unwrap();
        if let Ok(Some(frame)) = dec.decode(&encoded.data) {
            assert_eq!((frame.width, frame.height), (W, H));
            pictures += 1;
        }
    }
    assert!(
        pictures >= 5,
        "expected nearly every frame to decode, got {pictures}"
    );
}

#[test]
fn keyframes_are_larger_than_delta_frames() {
    // Sanity check that IDRs really are intra-coded: with a mostly static
    // scene an IDR must be substantially bigger than a delta frame.
    let mut enc = encoder();
    let mut cap = SyntheticCapturer::new(W, H);

    let idr = enc.encode(&cap.grab().unwrap()).unwrap();
    let delta = enc.encode(&cap.grab().unwrap()).unwrap();

    assert!(idr.is_keyframe && !delta.is_keyframe);
    assert!(
        idr.data.len() > delta.data.len(),
        "IDR ({} bytes) should exceed delta ({} bytes)",
        idr.data.len(),
        delta.data.len()
    );
}
