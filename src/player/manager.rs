//! [`Player`] event dispatch and management.

use super::Player;

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use twilight_gateway::Cluster;
use twilight_http::Client;
use twilight_model::voice::VoiceState;
use twilight_model::gateway::payload::incoming::{VoiceServerUpdate, VoiceStateUpdate};
use twilight_model::gateway::payload::outgoing::UpdateVoiceState;
use twilight_model::id::{
    marker::{ChannelMarker, GuildMarker, UserMarker},
    Id,
};

/// [`Player`] manager across many guilds.
///
/// This should be made for each shard or cluster.
#[derive(Clone)]
pub struct Manager {
    user_id: Id<UserMarker>,
    mref: Arc<ManagerRef>,
    http_client: Arc<Client>,
    gateway: Arc<Cluster>,
}

struct ManagerRef {
    players: RwLock<HashMap<Id<GuildMarker>, Player>>,
}

impl Manager {
    /// Creates a new manager.
    pub fn new(
        user_id: Id<UserMarker>,
        http_client: Arc<Client>,
        gateway: Arc<Cluster>,
    ) -> Manager {
        Manager {
            user_id,
            mref: Arc::new(ManagerRef {
                players: RwLock::new(HashMap::new()),
            }),
            http_client,
            gateway,
        }
    }

    /// Gets the player currently playing in a voice guild.
    ///
    /// Returns `None` if it does not exist or the player is closed.
    pub async fn get(&self, guild_id: impl Into<Id<GuildMarker>>) -> Option<Player> {
        self.get_player(guild_id.into()).await
    }

    /// Joins a voice channel in a guild.
    ///
    /// **If you call this on the event handler thread, it will deadlock!**
    /// Well, no, it actually wont, it times out after a while, but it won't
    /// work and that's it.
    pub async fn join(
        &self,
        guild_id: impl Into<Id<GuildMarker>>,
        channel_id: impl Into<Id<ChannelMarker>>,
    ) -> Player {
        let guild_id = guild_id.into();
        let channel_id = channel_id.into();

        // get or create player
        let player = match self.get_player(guild_id).await {
            Some(player) => player,
            None => self.new_player(guild_id).await,
        };

        // update voice state
        self.gateway
            .command(
                0,
                &UpdateVoiceState::new(guild_id, channel_id, false, false),
            )
            .await
            .unwrap();

        // return new player
        player
    }

    /// Consumes a voice state update.
    pub async fn voice_state_update(&self, ev: Box<VoiceStateUpdate>) {
        // This should be fine if we are only connected to a guild.
        let guild_id = ev.0.guild_id.unwrap();

        let players = self.mref.players.read().await;
        if let Some(player) = players.get(&guild_id) {
            let _ = player.voice_state_update(ev);
        }
    }

    /// Consumes a voice server update.
    pub async fn voice_server_update(&self, ev: VoiceServerUpdate) {
        let guild_id = ev.guild_id;

        let players = self.mref.players.read().await;
        if let Some(player) = players.get(&guild_id) {
            let _ = player.voice_server_update(ev);
        }
    }

    /// Creates a new player and adds it to the collection.
    ///
    /// Returns `None` if it does not exist or the player is closed.
    async fn new_player(&self, guild_id: Id<GuildMarker>) -> Player {
        // we don't have to provide an accurate state, just something to satisfy
        // the rust type checker
        let state = VoiceState {
            channel_id: None,
            guild_id: Some(guild_id),
            user_id: self.user_id,
            deaf: false,
            mute: false,
            self_deaf: false,
            self_mute: false,
            self_stream: false,
            self_video: false,
            suppress: false,
            session_id: String::new(),
            member: None,
            request_to_speak_timestamp: None,
        };

        let player = Player::new(self.http_client.clone(), self.user_id, guild_id, state).await;

        self.mref
            .players
            .write()
            .await
            .insert(guild_id, player.clone());

        player
    }

    /// Gets a player.
    ///
    /// Returns `None` if it does not exist or the player is closed.
    async fn get_player(&self, id: Id<GuildMarker>) -> Option<Player> {
        self.mref.players.read().await.get(&id).and_then(|p| {
            if p.is_closed() {
                None
            } else {
                Some(p.clone())
            }
        })
    }
}
