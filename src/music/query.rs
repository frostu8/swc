//! Offloads query work to other tasks.
//! 
//! `youtube-dl` takes a notoriously long time to query youtube for track info.

use tokio::sync::mpsc::{UnboundedSender, UnboundedReceiver, unbounded_channel};

use twilight_http::Client as HttpClient;

use std::future::Future;
use std::sync::Arc;

use super::commands::CommandData;

/// A query queue.
pub struct QueryQueue<T> {
    http_client: Arc<HttpClient>,

    query_tx: UnboundedSender<QueryResult<T>>,
    query_rx: UnboundedReceiver<QueryResult<T>>,
}

impl<T> QueryQueue<T>
where
    T: Send + 'static,
{
    /// Creates a new async query queue.
    pub fn new(http_client: Arc<HttpClient>) -> QueryQueue<T> {
        let (query_tx, query_rx) = unbounded_channel();

        QueryQueue {
            http_client,
            query_tx,
            query_rx,
        }
    }

    /// Enqueues a new task on the query queue.
    /// 
    /// When the result is ready, it will be retrieved with
    /// [`QueryQueue::next`].
    pub async fn enqueue<F, Fut>(&self, data: CommandData, task: F)
    where
        F: FnOnce(&CommandData) -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send,
    {
        let http_client = self.http_client.clone();
        let query_tx = self.query_tx.clone();

        tokio::spawn(async move {
            // ack response
            data
                .respond(&http_client)
                .ack()
                .await
                .unwrap();

            let result = task(&data).await;

            query_tx.send(QueryResult {
                data,
                message: result,
            }).unwrap();
        });
    }

    /// Fetches the next ready result.
    pub async fn next(&mut self) -> QueryResult<T> {
        self.query_rx.recv().await.unwrap()
    }
}

pub struct QueryResult<T> {
    pub data: CommandData,
    pub message: T,
}
