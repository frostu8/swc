//! Audio sources.
//!
//! Currently, this only supports ffmpeg and ytdl queries through an ffmpeg
//! pipe.
//! 
//! These should not be doing any super heavy CPU-bound work, as this runs on
//! the player thread. All of these features are cancel-safe.

use super::constants::{DEFAULT_BITRATE, SAMPLE_RATE, STEREO_FRAME_SIZE};

use crate::ytdl::YtdlError;

use tokio::process::{Child, Command};
use tokio::io::AsyncReadExt;

use std::process::Stdio;
use std::fmt::{self, Debug, Display, Formatter};

use opus::{Application, Encoder, Channels};

/// A ytdl audio source.
///
/// Encodes PCM32f @ 48000kHz into Opus-encoded audio. It's better to leave most
/// of the coding to ffmpeg, or another process, and that's what this does.
pub struct Source {
    piped: Option<Child>,
    ffmpeg: Child,

    coder: Encoder,
    buf: [f32; STEREO_FRAME_SIZE],
    buf_len: usize,
}

impl Source {
    /// Reads the next Opus packet into the buffer.
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        loop {
            let len = self
                .ffmpeg
                .stdout
                .as_mut()
                .unwrap()
                .read(bytemuck::cast_slice_mut(&mut self.buf[self.buf_len..]))
                .await
                .map_err(Error::Io)?;

            if len > 0 {
                self.buf_len += len / std::mem::size_of::<f32>();
                if self.buf_len >= self.buf.len() {
                    break;
                }
            } else {
                return Ok(0);
            }
        }

        if self.buf_len > 0 {
            // encode
            let len = self.coder.encode_float(&self.buf[..self.buf_len], buf).map_err(Error::Codec)?;
            self.buf_len = 0;
            Ok(len)
        } else {
            Ok(0)
        }
    }

    /// Kills the processes associated with the `Source`.
    pub async fn close(&mut self) -> Result<(), Error> {
        if let Some(mut piped) = self.piped.take() {
            piped.kill().await.map_err(Error::Io)?;
        }
        self.ffmpeg.kill().await.map_err(Error::Io)?;
        Ok(())
    }

    /// Creates a new `Source` from a process that produces audio (probably
    /// `ytdl`) and pipes it to `ffmpeg`.
    ///
    /// # Panics
    /// Panics if the process's `stdout` [`Stdio`] is not available. Remember
    /// to set the process's `stdout` to [`Stdio::piped`].
    ///
    /// ```no_run
    /// let mut ytdl = Command::new("youtube-dl")
    ///     .args(&[
    ///         "-f",
    ///         "webm[abr>0]/bestaudio/best",
    ///         "-R",
    ///         "infinite",
    ///         "-q",
    ///         query,
    ///         "-o",
    ///         "-",
    ///     ])
    ///     // remember to set stdout to piped! 
    ///     .stdout(Stdio::piped())
    ///     .stderr(Stdio::inherit())
    ///     .spawn()
    ///     .map_err(Error::Io)?;
    /// ```
    pub fn piped(mut piped: Child) -> Result<Source, Error> {
        let piped_stdio: Stdio = piped.stdout.take().unwrap().try_into().unwrap();

        let ffmpeg = Command::new("ffmpeg")
            .args(&[
                "-i",
                "pipe:0",
                "-ac",
                "2",
                "-ar",
                "48000",
                "-f",
                "s16le",
                "-acodec",
                "pcm_f32le",
                "-loglevel",
                "quiet",
                "pipe:1",
            ])
            .stdin(piped_stdio)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(Error::Io)?;

        let mut coder = Encoder::new(
            SAMPLE_RATE as u32,
            Channels::Stereo,
            Application::Audio,
        ).map_err(Error::Codec)?;
        coder.set_bitrate(DEFAULT_BITRATE).map_err(Error::Codec)?;

        Ok(Source {
            piped: Some(piped),
            ffmpeg,
            coder,
            buf: [0f32; STEREO_FRAME_SIZE],
            buf_len: 0,
        })
    }

    /// Creates a new `Source` from a `ytdl` query.
    pub fn ytdl(query: &str) -> Result<Source, Error> {
        let ytdl = Command::new(crate::ytdl::ytdl_executable())
            .args(&[
                "-f",
                "webm[abr>0]/bestaudio/best",
                "-R",
                "infinite",
                "-q",
                query,
                "-o",
                "-",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(Error::Io)?;

        Source::piped(ytdl)
    }
}

impl Debug for Source {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("Source(_)")
    }
}

/// An audio track error.
#[derive(Debug)]
pub enum Error {
    /// Io error.
    Io(std::io::Error),
    /// Codec error.
    Codec(opus::Error),
    /// Error from `youtube-dl`.
    Ytdl(YtdlError),
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Error::Io(err) => Display::fmt(err, f),
            Error::Codec(err) => Display::fmt(err, f),
            Error::Ytdl(err) => Display::fmt(err, f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(err) => Some(err),
            Error::Codec(err) => Some(err),
            Error::Ytdl(err) => Some(err),
        }
    }
}

