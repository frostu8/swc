//! Audio streamer.

use super::constants::{TIMESTEP_LENGTH, VOICE_PACKET_MAX, SILENCE_FRAME};
use super::rtp::{Socket, Packet};
use super::{Source, Error};

use tokio::time::{Instant, Duration, sleep_until, timeout_at};

/// Audio packet streamer.
///
/// Most of the time, we receive audio data faster than its playback speed. This
/// is actually really awesome! Well, until you realize that sending packets at
/// 4x the actual speed of the audio is going to cause some buffer issues and
/// also make you tonight's biggest loser.
pub struct PacketStreamer {
    patience: Duration,

    source: Option<Source>,
    waiting_for_source: bool,

    packet: Packet<[u8; VOICE_PACKET_MAX]>,
    next_packet: Instant,
    ready: bool,

    silence_frames: usize,
}

impl PacketStreamer {
    /// Create a new, empty `PacketStreamer`.
    ///
    /// `patience` determines how much extra time the packet streamer will wait
    /// for audio data before considering there to be a break in the stream, so
    /// it can do proper audio interpolation. 200ms is a good default.
    pub fn new(patience: Duration) -> PacketStreamer {
        PacketStreamer {
            patience,
            source: None,
            waiting_for_source: true,
            packet: Packet::default(),
            next_packet: Instant::now(),
            ready: false,
            silence_frames: 0,
        }
    }

    /// Gives the streamer a new source to play.
    pub fn source(&mut self, source: Source) {
        self.wait_for_source();
        self.source = Some(source);
    }

    /// Checks if a source is present in the streamer.
    pub fn has_source(&self) -> bool {
        self.source.is_some()
    }

    /// Takes the inner [`Source`].
    pub fn take_source(&mut self) -> Option<Source> {
        self.wait_for_source();
        self.source.take()
    }

    /// Streams the inner audio over the [`Socket`], pacing the packets so they
    /// don't destroy Discord.
    ///
    /// This future is intended to be cancelled, as it will not return unless
    /// there's an error or the status of packet flow changes.
    pub async fn stream(
        &mut self,
        rtp: &mut Socket,
    ) -> Result<Status, Error> {
        loop {
            if self.ready {
                sleep_until(self.next_packet).await;

                // send packet
                rtp.send(&mut self.packet).await?;

                // setup for next packet
                self.packet = Packet::default();
                self.next_packet = self.next_packet + TIMESTEP_LENGTH;
                self.ready = false;
            } else {
                self.next(rtp.ssrc()).await?;
            }
        }
    }

    /// Polls for the next packet.
    ///
    /// This will mark the `self.ready` flag so that the read packet can now
    /// be processed.
    async fn next(&mut self, ssrc: u32) -> Result<Option<Status>, Error> {
        if self.silence_frames > 0 {
            self.silence_frames -= 1;

            // copy silence frame
            (&mut self.packet.payload_mut()[..SILENCE_FRAME.len()])
                .copy_from_slice(SILENCE_FRAME);

            self.packet.set_payload_len(SILENCE_FRAME.len());
            self.ready = true;

            // if there is no audio left to play
            if self.silence_frames == 0 && self.waiting_for_source {
                // return new status
                Ok(Some(Status::Stopped(ssrc)))
            } else {
                // continue normal execution
                Ok(None)
            }
        } else {
            // get from source
            self.next_from_source(ssrc).await?;

            Ok(None)
        }
    }

    /// Polls for the next packet from the source.
    ///
    /// This will wait until the source is ready.
    async fn next_from_source(&mut self, ssrc: u32) -> Result<Option<Status>, Error> {
        let Some(source) = self.source.as_mut() else {
            // there is no source, wait
            std::future::pending().await
        };

        let (len, end_wait) = if self.waiting_for_source {
            // we don't actually need to satisfy a strict packet time schedule,
            // since Discord is no longer expecting packets
            let len = source.read(self.packet.payload_mut()).await?;

            // resume normal playback when the audio source continues results
            (len, true)
        } else {
            // we have to timeout if the source takes too long so we can warn
            // RTP of the break in audio
            let res = timeout_at(
                self.next_packet + self.patience,
                source.read(self.packet.payload_mut()),
            ).await;

            match res {
                Ok(Ok(len)) => (len, false),
                Ok(Err(err)) => return Err(err.into()),
                Err(_) => {
                    let now = Instant::now();
                    warn!("overloaded! {}ms", (now - self.next_packet).as_millis());

                    self.wait_for_source();

                    // exit so we can start playing the silence frames
                    return Ok(None)
                }
            }
        };

        if len > 0 {
            self.packet.set_payload_len(len);
            self.ready = true;
        } else {
            // clean up
            self.take_source().unwrap().close().await?;
            self.wait_for_source();
        }

        // if the source is finally returning, we can send a start signal
        if end_wait {
            // reset interval so we can stream the packets
            self.next_packet = Instant::now();
            self.waiting_for_source = false;

            Ok(Some(Status::Started(ssrc)))
        } else {
            Ok(None)
        }
    }

    fn wait_for_source(&mut self) {
        if !self.waiting_for_source {
            self.waiting_for_source = true;
            self.silence_frames += 5;
        }
    }
}

/// An event that is returned from [`PacketStreamer::stream`] that is
/// informative on the status of the streamer.
pub enum Status {
    /// Packets have begun streaming, with the first packet's `ssrc`.
    Started(u32),
    /// There is a break in transmission, packets have stopped streaming, with
    /// the last packet's `ssrc`.
    Stopped(u32),
}

