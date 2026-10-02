//! Unit tests for the hardware codec layer.
//!
//! The colour conversion is tested properly because it is pure arithmetic and
//! wrong output would show up as a subtly discoloured remote desktop rather than
//! a failure. The Media Foundation paths need real hardware and a real display,
//! so they are exercised by the `hwencode` example instead.

#![allow(clippy::unwrap_used, reason = "unwrap is acceptable in test code")]
#![allow(clippy::expect_used, reason = "expect is acceptable in test code")]

use super::*;

/// Build a `width` x `height` BGRA buffer from a per-pixel function.
fn bgra(width: u32, height: u32, stride: usize, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut buf = vec![0u8; stride * height as usize];
    for y in 0..height {
        for x in 0..width {
            let o = y as usize * stride + x as usize * 4;
            buf[o..o + 4].copy_from_slice(&f(x, y));
        }
    }
    buf
}

/// Y plane value at a pixel.
fn y_at(nv12: &[u8], width: u32, x: u32, y: u32) -> u8 {
    nv12[y as usize * width as usize + x as usize]
}

/// (U, V) for the 2x2 block containing a pixel.
fn uv_at(nv12: &[u8], width: u32, height: u32, x: u32, y: u32) -> (u8, u8) {
    let y_len = width as usize * height as usize;
    let row = (y / 2) as usize;
    let col = ((x / 2) * 2) as usize;
    let o = y_len + row * width as usize + col;
    (nv12[o], nv12[o + 1])
}

#[test]
fn rejects_unusable_geometry() {
    let mut dst = vec![0u8; 4096];
    // Odd dimensions have no 4:2:0 representation.
    assert!(bgra_to_nv12(&[0; 4096], 15, 16, 60, &mut dst).is_none());
    assert!(bgra_to_nv12(&[0; 4096], 16, 15, 64, &mut dst).is_none());
    // Zero sizes.
    assert!(bgra_to_nv12(&[0; 4096], 0, 16, 0, &mut dst).is_none());
    // Stride narrower than the row.
    assert!(bgra_to_nv12(&[0; 4096], 16, 16, 32, &mut dst).is_none());
    // Source too short for the declared geometry.
    assert!(bgra_to_nv12(&[0; 16], 16, 16, 64, &mut dst).is_none());
}

#[test]
fn rejects_short_destination() {
    let src = bgra(16, 16, 64, |_, _| [0, 0, 0, 255]);
    let mut dst = vec![0u8; nv12_len(16, 16) - 1];
    assert!(bgra_to_nv12(&src, 16, 16, 64, &mut dst).is_none());
}

#[test]
fn writes_the_expected_length() {
    let src = bgra(16, 8, 64, |_, _| [0, 0, 0, 255]);
    let mut dst = vec![0u8; nv12_len(16, 8)];
    let written = bgra_to_nv12(&src, 16, 8, 64, &mut dst).unwrap();
    assert_eq!(written, 16 * 8 * 3 / 2);
    assert_eq!(written, nv12_len(16, 8));
}

#[test]
fn black_and_white_hit_the_studio_swing_limits() {
    // BT.601 studio swing puts black at Y=16 and white at Y=235. Getting this
    // wrong produces a washed-out or crushed picture after decoding, which is
    // easy to miss by eye and trivial to assert.
    let black = bgra(16, 16, 64, |_, _| [0, 0, 0, 255]);
    let mut dst = vec![0u8; nv12_len(16, 16)];
    let _ = bgra_to_nv12(&black, 16, 16, 64, &mut dst).unwrap();
    assert_eq!(y_at(&dst, 16, 4, 4), 16, "black must be Y=16");
    let (u, v) = uv_at(&dst, 16, 16, 4, 4);
    assert_eq!((u, v), (128, 128), "greyscale must be chroma-neutral");

    let white = bgra(16, 16, 64, |_, _| [255, 255, 255, 255]);
    let _ = bgra_to_nv12(&white, 16, 16, 64, &mut dst).unwrap();
    assert_eq!(y_at(&dst, 16, 4, 4), 235, "white must be Y=235");
    let (u, v) = uv_at(&dst, 16, 16, 4, 4);
    assert_eq!((u, v), (128, 128), "greyscale must be chroma-neutral");
}

#[test]
fn channel_order_is_bgra_not_rgba() {
    // The single most likely bug in this function is reading the channels in the
    // wrong order, which swaps red and blue. Pure red and pure blue sit at
    // opposite ends of both chroma axes, so they cannot be confused.
    let mut dst = vec![0u8; nv12_len(16, 16)];

    // BGRA pure red: B=0, G=0, R=255.
    let red = bgra(16, 16, 64, |_, _| [0, 0, 255, 255]);
    let _ = bgra_to_nv12(&red, 16, 16, 64, &mut dst).unwrap();
    let (ur, vr) = uv_at(&dst, 16, 16, 2, 2);
    let yr = y_at(&dst, 16, 2, 2);

    // BGRA pure blue: B=255, G=0, R=0.
    let blue = bgra(16, 16, 64, |_, _| [255, 0, 0, 255]);
    let _ = bgra_to_nv12(&blue, 16, 16, 64, &mut dst).unwrap();
    let (ub, vb) = uv_at(&dst, 16, 16, 2, 2);
    let yb = y_at(&dst, 16, 2, 2);

    // Red is the brighter of the two in luma (0.257 vs 0.098).
    assert!(
        yr > yb,
        "red (Y={yr}) must be brighter than blue (Y={yb}); channels are swapped"
    );
    // Red pushes V up and U down; blue does the opposite. Asserting the
    // *separation* rather than absolute thresholds states the actual intent —
    // that the two cannot be confused — and avoids depending on where BT.601
    // happens to round. Pure blue's V is 109.9, which a `< 110` bound would
    // reject for no good reason.
    assert!(
        i32::from(vr) - i32::from(vb) > 100,
        "V must separate red ({vr}) from blue ({vb}); channel order is wrong"
    );
    assert!(
        i32::from(ub) - i32::from(ur) > 100,
        "U must separate blue ({ub}) from red ({ur}); channel order is wrong"
    );
}

#[test]
fn row_padding_is_honoured() {
    // Captured frames are padded to the staging texture's row pitch. Reading at
    // width*4 instead of the stride would shear the image progressively, which
    // this catches by giving each row a distinct value.
    let stride = 16 * 4 + 48;
    let src = bgra(16, 16, stride, |_, y| {
        let v = u8::try_from(y * 16).unwrap_or(255);
        [v, v, v, 255]
    });
    let mut dst = vec![0u8; nv12_len(16, 16)];
    let _ = bgra_to_nv12(&src, 16, 16, stride, &mut dst).unwrap();

    // Greyscale input means Y should rise monotonically down the image.
    for y in 1..16u32 {
        let prev = y_at(&dst, 16, 8, y - 1);
        let cur = y_at(&dst, 16, 8, y);
        assert!(
            cur >= prev,
            "row {y} luma {cur} should not be below row {} luma {prev}; stride ignored?",
            y - 1
        );
    }
    assert!(
        y_at(&dst, 16, 8, 15) > y_at(&dst, 16, 8, 0),
        "gradient must survive conversion"
    );
}

#[test]
fn chroma_is_averaged_across_the_block() {
    // A 2x2 block that is half black and half white should land near the middle
    // in luma per pixel but neutral in chroma, proving the block is averaged
    // rather than point-sampled from one corner.
    let src = bgra(2, 2, 8, |x, _| {
        if x == 0 {
            [0, 0, 0, 255]
        } else {
            [255, 255, 255, 255]
        }
    });
    let mut dst = vec![0u8; nv12_len(2, 2)];
    let _ = bgra_to_nv12(&src, 2, 2, 8, &mut dst).unwrap();
    assert_eq!(y_at(&dst, 2, 0, 0), 16);
    assert_eq!(y_at(&dst, 2, 1, 0), 235);
    let (u, v) = uv_at(&dst, 2, 2, 0, 0);
    assert_eq!((u, v), (128, 128), "grey average must be chroma-neutral");
}

#[test]
fn input_layout_reports_conversion_cost() {
    assert!(!InputLayout::Bgra32.needs_cpu_conversion());
    assert!(InputLayout::Nv12.needs_cpu_conversion());
}

#[test]
fn enumeration_does_not_fail_on_this_platform() {
    // On Windows this exercises the real Media Foundation path; elsewhere it must
    // return an empty list rather than an error, so callers need no cfg.
    let encoders = available_h264_encoders().expect("enumeration must not error");
    if !cfg!(windows) {
        assert!(encoders.is_empty(), "no encoders expected off Windows");
    }
    // has_hardware_h264 must agree with the list it summarises.
    let any_hw = encoders.iter().any(|e| e.hardware);
    assert_eq!(has_hardware_h264().unwrap(), any_hw);
}

// `tracing-subscriber` is a dev-dependency used only by the examples, which turn
// on logging to watch the Media Foundation negotiation. The lib's own test target
// never touches it, so acknowledge it here rather than dropping a dependency the
// examples need.
#[cfg(windows)]
use tracing_subscriber as _;
