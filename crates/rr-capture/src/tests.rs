//! Unit tests for the capture layer.

#![allow(clippy::unwrap_used, reason = "unwrap is acceptable in test code")]
#![allow(clippy::expect_used, reason = "expect is acceptable in test code")]

use super::*;

#[test]
fn synthetic_reports_one_primary_monitor() {
    let cap = SyntheticCapturer::new(64, 32);
    let monitors = cap.monitors().expect("monitors");
    assert_eq!(monitors.len(), 1);
    let m = &monitors[0];
    assert!(m.primary);
    assert_eq!(m.width, 64);
    assert_eq!(m.height, 32);
}

#[test]
fn synthetic_rejects_unknown_monitor() {
    let mut cap = SyntheticCapturer::new(16, 16);
    let err = cap.select_monitor(999).unwrap_err();
    assert!(matches!(err, CaptureError::UnknownMonitor(999)));
    // The advertised id must be accepted.
    let id = cap.monitors().unwrap()[0].id;
    cap.select_monitor(id).expect("select advertised monitor");
}

#[test]
fn odd_dimensions_are_rounded_down_to_even() {
    // H.264 4:2:0 requires even dimensions; silently producing an odd frame
    // would fail deep inside the encoder instead of here.
    let cap = SyntheticCapturer::new(65, 33);
    let m = &cap.monitors().unwrap()[0];
    assert_eq!(m.width, 64);
    assert_eq!(m.height, 32);
}

#[test]
fn frames_are_well_formed_and_validate() {
    let mut cap = SyntheticCapturer::new(32, 16);
    let frame = cap.grab().expect("grab");
    assert_eq!(frame.width, 32);
    assert_eq!(frame.height, 16);
    assert_eq!(frame.format, PixelFormat::Rgba8);
    assert_eq!(frame.stride, 32 * 4);
    assert_eq!(frame.data.len(), frame.stride * 16);
    frame.validate().expect("frame must be self-consistent");
}

#[test]
fn sequence_increments_per_frame() {
    let mut cap = SyntheticCapturer::new(8, 8);
    let a = cap.grab().unwrap();
    let b = cap.grab().unwrap();
    let c = cap.grab().unwrap();
    assert_eq!((a.sequence, b.sequence, c.sequence), (0, 1, 2));
}

#[test]
fn pattern_matches_published_expectation() {
    // The generator and the test oracle must agree, otherwise downstream
    // encode/decode assertions are meaningless.
    let mut cap = SyntheticCapturer::new(16, 8);
    let frame = cap.grab().unwrap();
    for y in 0..frame.height {
        let row = frame.row(y).expect("row in range");
        for x in 0..frame.width {
            let expected = SyntheticCapturer::expected_pixel(frame.width, x, y, frame.sequence);
            let offset = x as usize * 4;
            assert_eq!(
                &row[offset..offset + 4],
                &expected,
                "pixel ({x},{y}) mismatch"
            );
        }
    }
}

#[test]
fn moving_bar_changes_between_frames() {
    // Successive frames must differ, so the encoder sees real motion rather
    // than a static image it can trivially skip.
    let mut cap = SyntheticCapturer::new(16, 8);
    let a = cap.grab().unwrap();
    let b = cap.grab().unwrap();
    assert_ne!(a.data, b.data, "frames must differ frame-to-frame");
}

#[test]
fn row_padding_is_respected() {
    // A consumer that assumes width*bpp instead of stride would read garbage;
    // this frame has deliberate padding to catch that.
    let mut cap = SyntheticCapturer::new(8, 4).with_row_padding(16);
    let frame = cap.grab().unwrap();
    assert_eq!(frame.stride, 8 * 4 + 16);
    assert_eq!(frame.data.len(), frame.stride * 4);
    frame.validate().expect("padded frame is valid");

    // `row()` must strip the padding and return exactly the visible pixels.
    let row = frame.row(0).expect("row 0");
    assert_eq!(row.len(), 8 * 4);
}

#[test]
fn row_out_of_range_returns_none() {
    let mut cap = SyntheticCapturer::new(8, 4);
    let frame = cap.grab().unwrap();
    assert!(frame.row(4).is_none(), "row index == height must be None");
    assert!(frame.row(99).is_none());
}

#[test]
fn validate_rejects_a_short_buffer() {
    let mut cap = SyntheticCapturer::new(8, 4);
    let mut frame = cap.grab().unwrap();
    frame.data.truncate(frame.data.len() - 1);
    let err = frame.validate().unwrap_err();
    assert!(matches!(err, CaptureError::InconsistentFrame { .. }));
}

#[test]
fn pixel_format_bytes_per_pixel() {
    assert_eq!(PixelFormat::Rgba8.bytes_per_pixel(), 4);
    assert_eq!(PixelFormat::Bgra8.bytes_per_pixel(), 4);
}
