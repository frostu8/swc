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
use twilight_model::channel::message::embed::EmbedThumbnail;

use std::sync::Arc;
use std::collections::{HashMap, VecDeque};
use std::fmt::{self, Display, Formatter};

use tokio::task::JoinHandle;
use tokio::sync::{
    RwLockReadGuard,
    mpsc::{self, UnboundedSender, UnboundedReceiver},
};

use super::voice::{self, Player, Source};

use crate::ytdl::{Query as YtdlQuery, Track, QueryError};

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

            track_queue: VecDeque::default(),
            playing: None,
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

    track_queue: VecDeque<Track>,
    playing: Option<Track>,
}

type QueryResult = Result<YtdlQuery, QueryError>;

struct PlayerState {
    player: Player,
    event_rx: UnboundedReceiver<voice::Event>,
}

impl QueueState {
    pub async fn handle_command(&mut self, command: Command) {
        let Command { data, action } = command;

        let res = match action {
            Action::Play(track) => self.play(&data, track).await,
            Action::Skip => self.skip(&data).await,
            Action::Queue => self.queue(&data).await,
        };

        if let Err(err) = res {
            let _ = data
                .respond(&self.queue_server.http_client)
                .error(err)
                .respond()
                .await;
        }
    }

    async fn play(
        &mut self,
        command: &CommandData,
        query: String,
    ) -> Result<(), UserError> {
        match self.check_user_in_channel(command.user_id).await {
            // user is in the same channel
            Ok(_) => (),
            // join user's channel
            Err(UserError::BotNotInChannel(channel_id)) => {
                self.join(channel_id).await;
            }
            Err(err) => {
                return Err(err);
            }
        }

        self.query_queue.enqueue(
            command.clone(),
            |_| async move {
                YtdlQuery::query(&query).await
            }
        ).await;

        Ok(())
    }

    async fn skip(&mut self, command: &CommandData) -> Result<(), UserError> {
        self.check_user_in_channel(command.user_id).await?;

        self.skip_track();

        if let Some(track) = self.track_queue.front() {
            let _ = command
                .respond(&self.queue_server.http_client)
                .embed(Embed {
                    description: Some(String::from("skipped track")),
                    ..track.as_embed()
                })
                .respond()
                .await;
        } else {
            let _ = command
                .respond(&self.queue_server.http_client)
                .content("skipped track, now playing nothing :(")
                .respond()
                .await;
        }

        Ok(())
    }

    async fn queue(&self, command: &CommandData) -> Result<(), UserError> {
        let mut description = self
            .playing
            .as_ref()
            .map(|track| format!("now playing [{}]({})", track.title, track.url))
            .unwrap_or_else(|| String::from("nothing currently playing"));

        // construct queue
        for (i, track) in self.track_queue.iter().enumerate() {
            description.push_str(&format!("\n{}. [{}]({})",
                i + 1,
                track.title,
                track.url));
        }

        let embed = Embed {
            author: None,
            // TODO: color
            color: Some(0xEE1428),
            description: Some(description),
            fields: Vec::new(),
            footer: None,
            image: None,
            kind: String::from("rich"),
            provider: None,
            thumbnail: self
                .playing
                .as_ref()
                .and_then(|playing| playing.thumbnail_url.clone())
                .map(|url| EmbedThumbnail {
                    url: url,
                    height: None,
                    width: None,
                    proxy_url: None,
                }),
            timestamp: None,
            title: None,
            url: self
                .playing
                .as_ref()
                .map(|playing| playing.url.clone()),
            video: None,
        };

        let _ = command
            .respond(&self.queue_server.http_client)
            .embed(embed)
            .respond()
            .await;

        Ok(())
    }

    /// Checks if a user can use a music control command.
    /// 
    /// A user can use a music control command if the user is in the same
    /// channel as the bot.
    async fn check_user_in_channel(
        &self,
        user_id: Id<UserMarker>,
    ) -> Result<(), UserError>{
        let user_channel_id = self
            .queue_server
            .cache
            .voice_state(user_id, self.guild_id)
            .map(|s| s.channel_id());

        let voice_state = self.voice_state().await;
        if let Some(voice_state) = voice_state {
            if voice_state.channel_id == user_channel_id {
                Ok(())
            } else {
                Err(UserError::UserInDifferentChannel)
            }
        } else if let Some(channel_id) = user_channel_id {
            Err(UserError::BotNotInChannel(channel_id))
        } else {
            Err(UserError::UserNotInChannel)
        }
    }

    pub async fn handle_query(&mut self, result: QueryMessage<QueryResult>) {
        let QueryMessage {
            data: command,
            message,
        } = result;

        match message {
            Ok(query) => self.play_after_query(&command, query).await,
            Err(err) => {
                let _ = command
                    .respond(&self.queue_server.http_client)
                    .error(format!("failed to query: {}", err))
                    .update()
                    .await;
            }
        }
    }

    /// Executes the final result of a play command and their query.
    async fn play_after_query(&mut self, command: &CommandData, query: YtdlQuery) {
        match query {
            YtdlQuery::Track(track) => {
                let _ = command
                    .respond(&self.queue_server.http_client)
                    .embed(Embed {
                        description: Some(String::from("enqueued track")),
                        ..track.as_embed()
                    })
                    .update()
                    .await;

                // enqueue track
                self.place_track(track);
            }
        }
    }

    /// Enqueues a track onto the player.
    /// 
    /// Starts playing the song immediately if there is no song playing.
    /// Otherwise, enqueue the track on the queue.
    pub fn place_track(&mut self, track: Track) {
        if self.playing.is_none() {
            // get player
            let player = self.unwrap_player();

            // play track immediately
            player.play(Source::ytdl(&track.url).unwrap()).unwrap();

            self.playing = Some(track);
        } else {
            // place track on queue
            self.track_queue.push_back(track);
        }
    }

    /// Skips the current track by stopping the player.
    pub fn skip_track(&mut self) {
        let Some(PlayerState { player, .. }) = self.player.as_ref() else {
            return;
        };

        if player.playing() {
            player.stop().unwrap();
        }
    }

    /// Plays a new track onto the player.
    pub fn next_track(&mut self) {
        let Some(PlayerState { player, .. }) = self.player.as_ref() else {
            return;
        };

        if let Some(track) = self.track_queue.pop_back() {
            player.play(Source::ytdl(&track.url).unwrap()).unwrap();
            self.playing = Some(track);
        } else {
            self.playing = None;
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

    fn unwrap_player(&self) -> &Player {
        let PlayerState { player, .. } = self
            .player
            .as_ref()
            .expect("audio player");

        player
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
                    voice::EventType::Playing => {
                    }
                    voice::EventType::Stopped => {
                        // enqueue new track
                        state.next_track();
                    }
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

#[derive(Debug)]
enum UserError {
    UserInDifferentChannel,
    UserNotInChannel,
    BotNotInChannel(Id<ChannelMarker>)
}

impl Display for UserError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            UserError::UserInDifferentChannel => f.write_str(
                "you must be in the same voice channel as the bot to use \
                    this!"
            ),
            UserError::UserNotInChannel => f.write_str(
                "you must be in a voice channel to use this!"
            ),
            UserError::BotNotInChannel(_) => f.write_str(
                "the bot must be in a voice channel to use this!"
            ),
        }
    }
}

impl std::error::Error for UserError {}
