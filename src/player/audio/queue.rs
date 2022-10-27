//! Audio music queue.

use std::collections::VecDeque;

use super::Track;

/// A triple-buffer music queue.
///
/// [`Queue::next`] doesn't actually remove the track. Instead, the track
/// moves into the graveyard of the queue. If the graveyard gets too large
/// (determined by `keep_count` of [`Queue::new`]), it is then disposed of. This is to
/// support quick backtracking in the queue.
pub struct Queue {
    queue: VecDeque<Track>,
    head: usize,

    // amount to keep in the graveyard
    keep_count: usize,
}

impl Queue {
    /// Creates a new, empty `Queue`.
    pub fn new(keep_count: usize) -> Queue {
        Queue {
            queue: VecDeque::new(),
            head: 0,

            keep_count,
        }
    }

    /// Adds a new track to the back queue.
    pub fn push(&mut self, track: Track) {
        self.queue.push_back(track);
    }

    /// Advances the queue, returning the [`Track`] that was pushed into the
    /// graveyard.
    pub fn next(&mut self) -> Option<&Track> {
        if self.head >= self.queue.len() {
            return None;
        }

        if self.head >= self.keep_count {
            // remove the last song in the graveyard, effectively moving the
            // queue relative to the head
            self.queue.pop_front();
        } else {
            // move the head relative to the queue
            self.head += 1;
        }

        // return track
        Some(&self.queue[self.head - 1])
    }

    /// Backs up the queue, returning the [`Track`] now pulled out of the
    /// graveyard.
    pub async fn prev(&mut self) -> Option<&Track> {
        if self.head > 0 {
            // move the head relative to the queue
            self.head -= 1;

            // head <= queue.len()
            Some(&self.queue[self.head])
        } else {
            None
        }
    }
}

impl Default for Queue {
    fn default() -> Queue {
        Queue::new(15)
    }
}
