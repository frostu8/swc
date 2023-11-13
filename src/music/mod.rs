//! Higher level music queue interactions.
//!
//! There are lots of moving parts in this, but all of these moving parts are
//! self-contained within their own guilds, so for each guild, a task is spun
//! up, and commands are simply sent to each task, where the side-effect-doing
//! happens on the task. See [`Queue`] for more info.

mod commands;
mod query;

pub use commands::{Action, Command, CommandData};

use query::{QueryQueue, QueryResult as QueryMessage};
use twilight_model::channel::message::Embed;

use std::sync::Arc;
use std::collections::HashMap;

use tokio::task::JoinHandle;
use tokio::sync::{
    RwLockReadGuard,
    mpsc::{self, UnboundedSender, UnboundedReceiver},
};

use super::voice::{self, Player, Source};

use crate::ytdl::{Query as YtdlQuery, QueryError};

use twilight_gateway::MessageSender as GatewayMessageSender;
use twilight_http::Client as HttpClient;
use twilight_cache_inmemory::InMemoryCache;
use twilight_model::{
    voice::VoiceState,
    gateway::payload::{
        incoming::{VoiceServerUpdate, VoiceStateUpdate},
        outgoing::UpdateVoiceState,
    },
    id::{Id, marker::{ChannelMarker, GuildMarker, UserMarker}},
};

use tokio::sync::RwLock;

/// A music server is a shardable server for music queues.
pub struct QueueServer {
    gateway: GatewayMessageSender,
    cache: Arc<InMemoryCache>,
    http_client: Arc<HttpClient>,

    user_id: Id<UserMarker>,
    queues: RwLock<HashMap<Id<GuildMarker>, Queue>>,
}

impl QueueServer {
    /// Creates a new `QueueServer`.
    pub fn new(
        gateway: GatewayMessageSender,
        cache: Arc<InMemoryCache>,
        http_client: Arc<HttpClient>,

        user_id: Id<UserMarker>,
    ) -> QueueServer {
        QueueServer {
            gateway,
            http_client,
            cache,

            user_id,
            queues: RwLock::new(HashMap::new()),
        }
    }

    /// Sends a command to a queue in a guild.
    pub async fn command(
        self: &Arc<QueueServer>,
        guild_id: impl Into<Id<GuildMarker>>,
        command: Command,
    ) {
        self.with_queue(guild_id.into(), |queue| {
            let _ = queue.command_tx.send(command);
        }).await;
    }

    /// Processes a voice state update event from the gateway.
    pub async fn voice_state_update(self: &Arc<QueueServer>, ev: Box<VoiceStateUpdate>) {
        let Some(guild_id) = ev.guild_id else {
            return;
        };

        self.with_queue(guild_id, |queue| {
            let _ = queue.gateway_tx.send(GatewayEvent::VoiceStateUpdate(ev));
        }).await;
    }

    /// Processes a voice server update event from the gateway.
    pub async fn voice_server_update(self: &Arc<QueueServer>, ev: VoiceServerUpdate) {
        self.with_queue(ev.guild_id, |queue| {
            let _ = queue.gateway_tx.send(GatewayEvent::VoiceServerUpdate(ev));
        }).await;
    }

    /// Gets a currently running queue or starts a new queue.
    async fn with_queue<F>(self: &Arc<QueueServer>, guild_id: Id<GuildMarker>, f: F)
    where
        F: FnOnce(&Queue),
    {
        let queues = self.queues.read().await;

        if let Some(queue) = queues.get(&guild_id) {
            if !queue.task.is_finished() {
                f(queue);
                return;
            }
        }

        // start a new queue
        let new_queue = Queue::new(self.clone(), guild_id);

        f(&new_queue);

        // DROP THE FUCKING RWLOCK GUARD, THIS CAUSES A DEADLOCK.
        drop(queues);

        // add to map
        let mut queues = self.queues.write().await;
        queues.insert(guild_id, new_queue);
    }
}

/// A single music queue.
struct Queue {
    task: JoinHandle<()>,
    command_tx: UnboundedSender<Command>,
    gateway_tx: UnboundedSender<GatewayEvent>,
}

#[derive(Debug)]
enum GatewayEvent {
    VoiceStateUpdate(Box<VoiceStateUpdate>),
    VoiceServerUpdate(VoiceServerUpdate),
}

impl Queue {
    /// Spins up a new queue task.
    pub fn new(
        queue_server: Arc<QueueServer>,
        guild_id: impl Into<Id<GuildMarker>>,
    ) -> Queue {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (gateway_tx, gateway_rx) = mpsc::unbounded_channel();

        // start task
        let task = tokio::spawn(queue_run(QueueState {
            query_queue: QueryQueue::new(queue_server.http_client.clone()),

            queue_server,
            guild_id: guild_id.into(),

            player: None,
            command_rx,
            gateway_rx,
        }));

        Queue {
            task,
            command_tx,
            gateway_tx,
        }
    }
}

struct QueueState {
    queue_server: Arc<QueueServer>,
    guild_id: Id<GuildMarker>,

    player: Option<PlayerState>,
    query_queue: QueryQueue<QueryResult>,
    command_rx: UnboundedReceiver<Command>,
    gateway_rx: UnboundedReceiver<GatewayEvent>,
}

type QueryResult = Result<YtdlQuery, QueryError>;

struct PlayerState {
    player: Player,
    event_rx: UnboundedReceiver<voice::Event>,
}

impl QueueState {
    pub async fn handle_command(&mut self, command: Command) {
        let Command { data, action } = command;

        match action {
            Action::Play(track) => {
                self.ensure_voice(&data).await;

                self.query_queue.enqueue(
                    data,
                    |_| async move {
                        YtdlQuery::query(&track).await
                    }
                ).await;
            },
            Action::Skip => todo!()
        }
    }

    pub async fn handle_query(&mut self, result: QueryMessage<QueryResult>) {
        let QueryMessage {
            data: command,
            message,
        } = result;

        match message {
            Ok(query) => self.play(&command, query).await,
            Err(err) => {
                let _ = command
                    .respond(&self.queue_server.http_client)
                    .error(format!("failed to query: {}", err))
                    .update()
                    .await;
            }
        }
    }

    pub async fn ensure_voice(&mut self, command: &CommandData) {
        let user_channel_id = self
            .queue_server
            .cache
            .voice_state(command.user_id, self.guild_id)
            .map(|s| s.channel_id());

        let voice_state = self.voice_state().await;
        if let Some(voice_state) = voice_state {
            if voice_state.channel_id != user_channel_id {
                command
                    .respond(&self.queue_server.http_client)
                    .error("you must be in the same voice channel as the bot\
                        to use this!")
                    .respond()
                    .await
                    .unwrap();

                return;
            }
        } else if let Some(channel_id) = user_channel_id {
            drop(voice_state);
            // join user's channel
            self.join(channel_id).await;
        } else {
            command
                .respond(&self.queue_server.http_client)
                .error("you must be in a voice channel to use this!")
                .respond()
                .await
                .unwrap();

            return;
        }
    }

    pub async fn play<'a>(&mut self, command: &'a CommandData, query: YtdlQuery) {
        // get player
        let PlayerState { player, .. } = self.player.as_ref().expect("audio player");

        match query {
            YtdlQuery::Track(track) => {
                // TODO: queue
                // for now, place the song directly on the player
                player.play(Source::ytdl(&track.url).unwrap()).unwrap();

                let _ = command
                    .respond(&self.queue_server.http_client)
                    .embed(Embed {
                        description: Some(String::from("enqueued track")),
                        ..track.as_embed()
                    })
                    .update()
                    .await;
            }
        }
    }

    /// Returns the current voice state of the bot, or `None` if there is no
    /// current state (the player is closed or None).
    pub async fn voice_state(&self) -> Option<RwLockReadGuard<VoiceState>> {
        if let Some(PlayerState { player, .. }) = self.player.as_ref() {
            player.voice_state().await.ok()
        } else {
            None
        }
    }

    /// Joins or moves the bot to a Discord channel.
    pub async fn join(&mut self, channel_id: Id<ChannelMarker>) {
        let voice_state = self.voice_state().await;
        if let Some(voice_state) = voice_state {
            if voice_state.channel_id == Some(channel_id) {
                // we are already in the channel, return
                return;
            }
        } else {
            // rust is kind of weird, but I might just be stupid
            drop(voice_state);
            // there is no player
            self.start_player();
        }

        // a player is definitely running now, send voice state event
        // update voice state
        self.queue_server
            .gateway
            .command(
                &UpdateVoiceState::new(self.guild_id, channel_id, false, false),
            )
            .unwrap();
    }

    fn start_player(&mut self) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let player = Player::new(
            self.queue_server.user_id,
            self.guild_id,
            event_tx,
        );

        self.player = Some(PlayerState {
            player,
            event_rx,
        });
    }
}

async fn queue_run(mut state: QueueState) {
    loop {
        tokio::select! {
            biased;

            // high level command
            Some(command) = state.command_rx.recv() => {
                state.handle_command(command).await;
            }
            // high level queue event
            message = state.query_queue.next() => {
                state.handle_query(message).await;
            }
            // gateway event
            Some(event) = state.gateway_rx.recv() => {
                debug!("{:?}", event);

                if let Some(PlayerState { player, .. }) = state.player.as_mut() {
                    let _ = match event {
                        GatewayEvent::VoiceStateUpdate(ev) => player.voice_state_update(ev),
                        GatewayEvent::VoiceServerUpdate(ev) => player.voice_server_update(ev),
                    };
                }
            },
            // low level voice event
            Some(event) = voice_event(state.player.as_mut()) => {
                debug!("{:?}", event);

                match event.kind {
                    voice::EventType::Ready => {
                    }
                    voice::EventType::Error(err) => {
                        error!("audio: {}", err);
                    }
                    _ => ()
                };
            }
        }
    }
}

async fn voice_event(player: Option<&mut PlayerState>) -> Option<voice::Event> {
    if let Some(player) = player {
        player.event_rx.recv().await
    } else {
        std::future::pending().await
    }
}

