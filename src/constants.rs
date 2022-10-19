//! Constants.

/// Raw sample rate of Discord Opus stream.
pub const SAMPLE_RATE_RAW: usize = 48_000;

/// The rate of frames to be sent per second.
pub const AUDIO_FRAME_RATE: usize = 50;

/// Number of samples in one complete frame of audio per channel.
pub const MONO_FRAME_SIZE: usize = SAMPLE_RATE_RAW / AUDIO_FRAME_RATE;
