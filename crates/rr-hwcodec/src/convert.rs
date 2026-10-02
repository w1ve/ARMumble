//! BGRA to NV12 colour conversion.
//!
//! Needed only when the encoder will not take BGRA directly. An MFT that accepts
//! BGRA converts on the GPU as part of encoding, which is free to us; one that
//! insists on NV12 makes us pay here, and at 3840x2160 that is 8.3 million pixels
//! of work per frame. [`crate::InputLayout`] records which case we are in so the
//! cost is visible rather than hidden.
//!
//! Contains **no `unsafe`**, despite living in the crate that is allowed it:
//! this is ordinary arithmetic over slices and there is no reason to reach for
//! raw pointers.
//!
//! # Colour space
//!
//! BT.601 studio swing (Y in 16–235, chroma in 16–240), which is what the H.264
//! encoder expects by default for standard-definition-derived coefficients and
//! what Media Foundation assumes absent explicit colour metadata. Using full
//! range here would wash out the picture after decoding.

/// Clamp a rounded fixed-point intermediate into a byte.
fn clamp_u8(v: i32) -> u8 {
    // `u8::try_from` would need an error path for a value we simply want to
    // saturate, so clamp explicitly.
    v.clamp(0, 255) as u8
}

/// Convert packed BGRA to NV12.
///
/// `src` is `stride * height` bytes of BGRA. `stride` may exceed `width * 4`, as
/// captured frames are often padded. `dst` receives `width * height * 3 / 2`
/// bytes: a full-resolution Y plane followed by an interleaved, 2x2-subsampled
/// UV plane.
///
/// Returns the number of bytes written, or `None` if the geometry is unusable or
/// a buffer is too small. Dimensions must be even, as 4:2:0 subsampling has no
/// meaning otherwise.
#[must_use]
pub fn bgra_to_nv12(
    src: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    dst: &mut [u8],
) -> Option<usize> {
    to_nv12::<true>(src, width, height, stride, dst)
}

/// Convert packed RGBA to NV12.
///
/// Same as [`bgra_to_nv12`] but for the opposite channel order, which is what the
/// portable `xcap` capture backend produces. Having both avoids a separate
/// full-frame swizzle pass just to reuse one function.
#[must_use]
pub fn rgba_to_nv12(
    src: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    dst: &mut [u8],
) -> Option<usize> {
    to_nv12::<false>(src, width, height, stride, dst)
}

/// The shared implementation.
///
/// `BGRA` is a const parameter rather than a runtime flag so the channel indices
/// fold away at compile time and the inner loop is identical to a hand-written
/// single-order version.
fn to_nv12<const BGRA: bool>(
    src: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    dst: &mut [u8],
) -> Option<usize> {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return None;
    }
    if stride < w * 4 || src.len() < stride * h {
        return None;
    }
    let y_len = w * h;
    let needed = y_len + y_len / 2;
    if dst.len() < needed {
        return None;
    }

    // Split once, so the two planes can be written without bounds checks on
    // every pixel and without aliasing concerns.
    let (y_plane, uv_plane) = dst.split_at_mut(y_len);

    // Two rows at a time: chroma is shared by each 2x2 block, so pairing the rows
    // means each block's four pixels are averaged in one pass instead of being
    // re-read for a second chroma pass.
    for by in 0..h / 2 {
        let y0 = by * 2;
        let row0 = &src[y0 * stride..y0 * stride + w * 4];
        let row1 = &src[(y0 + 1) * stride..(y0 + 1) * stride + w * 4];
        let (y_row0, y_row1) = y_plane.get_mut(y0 * w..(y0 + 2) * w)?.split_at_mut(w);
        let uv_row = uv_plane.get_mut(by * w..(by + 1) * w)?;

        for bx in 0..w / 2 {
            let x = bx * 2;
            let mut b_sum = 0i32;
            let mut g_sum = 0i32;
            let mut r_sum = 0i32;

            // Channel offsets, resolved at compile time from the const parameter.
            let (bi, ri) = if BGRA { (0usize, 2usize) } else { (2usize, 0usize) };

            // The four pixels of this 2x2 block: luma per pixel, chroma averaged.
            for (row, y_row) in [(row0, &mut *y_row0), (row1, &mut *y_row1)] {
                for k in 0..2 {
                    let o = (x + k) * 4;
                    let b = i32::from(row[o + bi]);
                    let g = i32::from(row[o + 1]);
                    let r = i32::from(row[o + ri]);
                    b_sum += b;
                    g_sum += g;
                    r_sum += r;
                    // Y = 0.257R + 0.504G + 0.098B + 16, in 16.16 fixed point.
                    let y = (16_843 * r + 33_030 * g + 6_423 * b + (16 << 16) + 32_768) >> 16;
                    y_row[x + k] = clamp_u8(y);
                }
            }

            // Average the block, then convert once. Rounding the sum of four
            // samples is both cheaper and slightly more accurate than converting
            // one corner pixel.
            let b = (b_sum + 2) / 4;
            let g = (g_sum + 2) / 4;
            let r = (r_sum + 2) / 4;
            // U = -0.148R - 0.291G + 0.439B + 128
            let u = (-9_699 * r - 19_071 * g + 28_770 * b + (128 << 16) + 32_768) >> 16;
            // V =  0.439R - 0.368G - 0.071B + 128
            let v = (28_770 * r - 24_117 * g - 4_653 * b + (128 << 16) + 32_768) >> 16;
            uv_row[x] = clamp_u8(u);
            uv_row[x + 1] = clamp_u8(v);
        }
    }

    Some(needed)
}

/// Bytes an NV12 buffer needs for the given geometry.
#[must_use]
pub const fn nv12_len(width: u32, height: u32) -> usize {
    let y = width as usize * height as usize;
    y + y / 2
}
