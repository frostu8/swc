//! Constants.

use std::time::Duration;

// Many of these constants were stolen from:
// https://github.com/serenity-rs/songbird/blob/a2f55b7a35539c00e3a75edfb01d1777e8b19741/src/constants.rs

/// Raw sample rate of Discord Opus stream.
pub const SAMPLE_RATE: usize = 48_000;

/// The rate of frames to be sent per second.
pub const AUDIO_FRAME_RATE: usize = 50;

/// Length of time between any two audio frames.
pub const TIMESTEP_LENGTH: Duration = Duration::from_millis(1000 / AUDIO_FRAME_RATE as u64);

/// Number of samples in one complete frame of audio per channel.
pub const MONO_FRAME_SIZE: usize = SAMPLE_RATE / AUDIO_FRAME_RATE;

/// Number of individual samples in one complete frame of stereo audio.
pub const STEREO_FRAME_SIZE: usize = 2 * MONO_FRAME_SIZE;

/// Number of bytes in one complete frame of raw `f32`-encoded mono audio.
pub const MONO_FRAME_BYTE_SIZE: usize = MONO_FRAME_SIZE * std::mem::size_of::<f32>();

/// Number of bytes in one complete frame of raw `f32`-encoded stereo audio.
pub const STEREO_FRAME_BYTE_SIZE: usize = STEREO_FRAME_SIZE * std::mem::size_of::<f32>();

/// Maximum packet size for a voice packet.
///
/// Set a safe amount below the Ethernet MTU to avoid fragmentation/rejection.
pub const VOICE_PACKET_MAX: usize = 1460;

