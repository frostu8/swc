//! Types helpful for interacting with the youtube-dl command line.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::process::{Command};

use std::process::Stdio;
use std::fmt::{self, Display, Formatter};
use std::sync::OnceLock;

use serde::Deserialize;

//use crate::voice::{Source, source::Error as SourceError};

static YTDL_EXECUTABLE: OnceLock<String> = OnceLock::new();

/// The `youtube-dl` executable.
pub fn ytdl_executable() -> &'static str {
    YTDL_EXECUTABLE.get().expect("ytdl executable initialized at startup")
}

pub fn init_ytdl_executable<F>(f: F) -> &'static str
where
    F: FnOnce() -> String
{
    YTDL_EXECUTABLE.get_or_init(f)
}

/// The result of a `youtube-dl` query.
pub enum Query {
    /// A track was found.
    Track(Track),
}

impl Query {
    /// Queries `youtube-dl` with the provided string.
    ///
    /// Also produces a [`Source`] if the result is a [`Track`].
    ///
    /// # Warning
    /// Because yt-dlp has to do some heavy networking overhead, this is a very
    /// slow operation, and has a tendency to time things out. Offload this
    /// work to a new async task and communicate the completion of the task
    /// through message passing.
    pub async fn query(query: &str) -> Result<Query, QueryError> {
        let mut ytdl = Command::new(ytdl_executable())
            .args(&["--yes-playlist", "--flat-playlist", "-J", query])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(QueryError::Io)?;

        let stdout = ytdl.stdout.take().unwrap();
        let stderr = ytdl.stderr.take().unwrap();

        async fn read_to_end(
            mut stream: impl AsyncRead + Unpin,
        ) -> Result<String, std::io::Error> {
            let mut out = String::new();
            stream.read_to_string(&mut out).await.map(|_| out)
        }

        // wait for the query to finish
        let (_exit, out, err) = tokio::try_join!(
            ytdl.wait(),
            read_to_end(stdout),
            YtdlError::from_ytdl(BufReader::new(stderr)),
        )
            .map_err(QueryError::Io)?;

        if let Some(err) = err {
            Err(QueryError::Ytdl(err))
        } else {
            // TODO: only supports single tracks with a source
            Query::track_from_json(&out)
        }
    }

    fn track_from_json(
        json: &str,
    ) -> Result<Query, QueryError> {
        // parse json data
        #[derive(Deserialize)]
        struct YtdlQuery {
            webpage_url: String,
            title: String,
            //uploader: String,
            /*
            #[serde(default)]
            thumbnail: Option<String>,
            #[serde(default)]
            uploader_url: Option<String>,
            */
        }

        let YtdlQuery {
            webpage_url,
            title,
        } = serde_json::from_str(json).map_err(QueryError::Json)?;

        // create a track as the result
        let track = Track {
            url: webpage_url,
            title,
        };

        Ok(Query::Track(track))
    }
}

/// A single `youtube-dl` track.
///
/// Produced from the output of a `youtube-dl` query.
pub struct Track {
    /// A url which, when provided to `youtube-dl` should produce the same
    /// result.
    pub url: String,
    /// A visible title for a song.
    pub title: String,
}

/// An error that can occur querying `youtube-dl`.
#[derive(Debug)]
pub enum QueryError {
    /// There was an IO error.
    Io(std::io::Error),
    /// UTF8 error while processing input JSON.
    Utf8(std::str::Utf8Error),
    /// Serialization error while processing input JSON.
    Json(serde_json::Error),
    /// Ytdl produced an error.
    Ytdl(YtdlError),
}

impl Display for QueryError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            QueryError::Io(err) => Display::fmt(err, f),
            QueryError::Utf8(err) => Display::fmt(err, f),
            QueryError::Json(err) => Display::fmt(err, f),
            QueryError::Ytdl(err) => Display::fmt(err, f),
        }
    }
}

impl std::error::Error for QueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            QueryError::Io(err) => Some(err),
            QueryError::Utf8(err) => Some(err),
            QueryError::Json(err) => Some(err),
            QueryError::Ytdl(err) => Some(err),
        }
    }
}

/// An error from a `youtube-dl` command.
#[derive(Debug)]
pub struct YtdlError {
    message: String,
}

impl YtdlError {
    /// Reads an error from a stream, most likely stdout of a `youtube-dl`
    /// process.
    ///
    /// If an error is not found, returns `None`.
    ///
    /// `youtube-dl` error codes are meaningless, so this is the only way we can
    /// get a message from `youtube-dl`.
    pub async fn from_ytdl<T>(stderr: T) -> Result<Option<YtdlError>, std::io::Error>
    where
        T: AsyncBufRead + Unpin,
    {
        // youtube-dl stderr looks like this:
        // WARNING: warning
        // ERROR: error <-- this is what we want
        const ERROR_PREFIX: &'static str = "ERROR:";

        let mut lines = stderr.lines();
        while let Some(line) = lines.next_line().await? {
            if line.starts_with(ERROR_PREFIX) {
                return Ok(Some(YtdlError {
                    message: line[ERROR_PREFIX.len()..].trim().to_owned(),
                }));
            }
        }

        Ok(None)
    }

    /// The message of the error.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Display for YtdlError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for YtdlError {}

