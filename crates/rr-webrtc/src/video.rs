//! Video track plumbing.
//!
//! The Station publishes one H.264 video track per viewer and writes encoded
//! access units into it; the Client receives the track and feeds the bitstream
//! to its decoder.
//!
//! # Why encoded samples, not raw frames
//!
//! We encode once in `rr-codec` and hand `WebRTC` the finished bitstream through
//! [`TrackLocalStaticSample`]. That keeps the codec configuration (screen-content
//! tuning, keyframe-on-demand) under our control instead of delegating it to a
//! built-in encoder we cannot tune for latency.
//!
//! `rr-capture` and `rr-codec` are dev-dependencies used by the end-to-end
//! `video_pipeline` integration test; the library itself deliberately stays
//! codec-agnostic and only moves opaque byte slices.
#[cfg(test)]
use rr_capture as _;
#[cfg(test)]
use rr_codec as _;

use std::sync::Arc;
use std::time::{Duration, Instant};

// `RtpCodecKind` is re-exported here; the `rtp_codec` module that defines it is
// private, so importing through it does not compile.
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use rtc::rtp_transceiver::{PayloadType, SSRC};
use rtc_media::Sample;
use tokio::sync::Mutex;
use tracing::debug;
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::media_stream::MediaStreamTrack;

use crate::{Result, TransportError};

/// Payload type for H.264, per the dynamic range used in our SDP.
///
/// 102 is within the dynamic payload-type range (96-127) and is the value
/// commonly used for H.264 by `WebRTC` implementations.
pub const H264_PAYLOAD_TYPE: PayloadType = 102;

/// Stream and track identifiers advertised for the screen-share track.
pub const VIDEO_STREAM_ID: &str = "rr-screen";
/// Track id for the screen-share video track.
pub const VIDEO_TRACK_ID: &str = "rr-video";

/// Nominal frame duration used when a caller does not supply one.
///
/// Only a hint for packetisation; actual pacing follows the capture loop.
const DEFAULT_FRAME_DURATION: Duration = Duration::from_millis(33);

/// The Station's outbound video track.
///
/// Wraps [`TrackLocalStaticSample`] with the bookkeeping needed to write
/// encoded frames: a fixed SSRC/payload type and the previous timestamp so
/// sample durations are sensible.
pub struct VideoSender {
    track: Arc<TrackLocalStaticSample>,
    ssrc: SSRC,
    last_written: Mutex<Option<Instant>>,
}

impl std::fmt::Debug for VideoSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoSender")
            .field("ssrc", &self.ssrc)
            .finish_non_exhaustive()
    }
}

impl VideoSender {
    /// Create the outbound H.264 video track.
    ///
    /// `ssrc` must be unique within the peer connection.
    ///
    /// # Errors
    /// Returns [`TransportError::WebRtc`] if the track cannot be created.
    pub fn new(ssrc: SSRC) -> Result<Self> {
        let media_track = MediaStreamTrack::new(
            VIDEO_STREAM_ID.to_owned(),
            VIDEO_TRACK_ID.to_owned(),
            "screen".to_owned(),
            RtpCodecKind::Video,
            Vec::new(),
        );
        let track = TrackLocalStaticSample::new(Instant::now(), media_track)
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        Ok(Self {
            track: Arc::new(track),
            ssrc,
            last_written: Mutex::new(None),
        })
    }

    /// The underlying track, for adding to a peer connection.
    #[must_use]
    pub fn track(&self) -> Arc<TrackLocalStaticSample> {
        Arc::clone(&self.track)
    }

    /// Write one encoded access unit to the track.
    ///
    /// The sample duration is derived from the gap since the previous write, so
    /// a capture loop running at a variable rate still produces sane timing.
    ///
    /// # Errors
    /// Returns [`TransportError::WebRtc`] if the track rejects the sample.
    pub async fn send_encoded(&self, data: &[u8]) -> Result<()> {
        let now = Instant::now();
        let duration = {
            let mut last = self.last_written.lock().await;
            let d = last.map_or(DEFAULT_FRAME_DURATION, |prev| now.duration_since(prev));
            *last = Some(now);
            // Guard against a zero/absurd duration from clock jitter or a
            // burst of frames written back-to-back.
            if d.is_zero() || d > Duration::from_secs(1) {
                DEFAULT_FRAME_DURATION
            } else {
                d
            }
        };

        // `Sample` has no `Default`, so start from `new` and fill in the
        // fields that matter for an encoded video access unit.
        let mut sample = Sample::new(now);
        sample.data = bytes::Bytes::copy_from_slice(data);
        sample.timestamp = now;
        sample.duration = duration;

        debug!(bytes = data.len(), ?duration, "writing video sample");
        self.track
            .sample_writer(self.ssrc, H264_PAYLOAD_TYPE)
            .write_sample(&sample)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))
    }
}

/// Reassembles inbound video for the Client.
///
/// `WebRTC` delivers RTP; this collects the payloads belonging to a frame and
/// hands complete access units to the application, which passes them to
/// `rr-codec`'s decoder.
#[derive(Debug, Default)]
pub struct VideoReceiver {
    /// Access units collected but not yet taken by the application.
    pending: Mutex<Vec<Vec<u8>>>,
}

impl VideoReceiver {
    /// Create an empty receiver.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a complete access unit received from the network.
    pub async fn push_access_unit(&self, data: Vec<u8>) {
        self.pending.lock().await.push(data);
    }

    /// Take all access units received so far, oldest first.
    pub async fn take_access_units(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut *self.pending.lock().await)
    }

    /// Number of access units waiting to be consumed.
    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.len()
    }
}

#[cfg(test)]
mod tests;
