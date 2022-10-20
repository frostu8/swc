//! Types for working with command line tool `youtube-dl`.

use tokio::io::{AsyncRead, AsyncBufReadExt, BufReader};

use std::fmt::{self, Display, Formatter};

/// An error from a `youtube-dl` command.
#[derive(Debug)]
pub struct Error {
    message: String,
}

impl Error {
    /// Reads an error from a stream.
    ///
    /// If an error is not found, returns `None`.
    ///
    /// `youtube-dl` error codes are meaningless, so this is the only way we can
    /// get a message from `youtube-dl`.
    pub async fn from_ytdl<T>(stream: T) -> Result<Option<Error>, std::io::Error>
    where
        T: AsyncRead + Unpin,
    {
        // youtube-dl stderr looks like this:
        // WARNING: warning
        // ERROR: error <-- this is what we want
        const ERROR_PREFIX: &'static str = "ERROR:";

        let stream = BufReader::new(stream);

        let mut lines = stream.lines();
        while let Some(line) = lines.next_line().await? {
            if line.starts_with(ERROR_PREFIX) {
                return Ok(Some(Error {
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

impl Display for Error {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}
