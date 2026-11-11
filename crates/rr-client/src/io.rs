//! The two audio endpoints handed to the audio backend.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use rr_audio::capture::{CaptureEngine, EncodedPacket};
use rr_audio::playout::Playout;
use rr_mumble::message::UdpMessage;
use rr_mumble::proto::mumble_udp::{self, audio::Header};

use crate::shared::Shared;

/// Everything the audio backend needs to run.
#[derive(Debug)]
pub struct AudioEndpoints {
    /// Feed microphone / line-in samples here from the input callback.
    pub input: InputPipe,
    /// Pull mixed audio from here in the output callback.
    pub output: Playout,
    /// A second, independent playout for a second output device, present
    /// when [`crate::ClientSettings::output_rate_b`] is set. It receives the
    /// same audio and drift-locks to its own device clock.
    pub output_b: Option<Playout>,
}

/// Input side: runs the capture engine and forwards packets to the network.
///
/// Call [`InputPipe::process`] from the audio input callback. It never blocks:
/// if the network side is busy the packet is counted as dropped.
#[derive(Debug)]
pub struct InputPipe {
    engine: CaptureEngine,
    shared: Arc<Shared>,
    packets: Vec<EncodedPacket>,
}

impl InputPipe {
    pub(crate) fn new(engine: CaptureEngine, shared: Arc<Shared>) -> Self {
        Self { engine, shared, packets: Vec::new() }
    }

    /// The device format the pipe currently expects: (rate, channels).
    #[must_use]
    pub fn device_format(&self) -> (u32, usize) {
        self.engine.device_format()
    }

    /// Switch to a capture device with another rate or channel count.
    pub fn set_device_format(&mut self, rate: u32, channels: usize) {
        self.engine.set_device_format(rate, channels);
    }

    /// Process interleaved device samples.
    pub fn process(&mut self, samples: &[f32]) {
        self.packets.clear();
        self.engine.process(samples, &mut self.packets);
        let target = self.shared.target.load(Ordering::Relaxed);
        for p in self.packets.drain(..) {
            let audio = mumble_udp::Audio {
                header: Some(Header::Target(target)),
                sender_session: 0,
                frame_number: p.seq,
                opus_data: p.data,
                positional_data: Vec::new(),
                volume_adjustment: 0.0,
                is_terminator: p.terminator,
            };
            let sent = self
                .shared
                .try_handle()
                .is_some_and(|h| h.send_audio(UdpMessage::Audio(audio)).is_ok());
            let counter = if sent { &self.shared.sent } else { &self.shared.dropped };
            let _ = counter.fetch_add(1, Ordering::Relaxed);
        }
    }
}
