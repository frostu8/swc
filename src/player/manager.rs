//! [`Player`] event dispatch and management.

use super::Player;

use std::sync::Arc;
use std::collections::HashMap;

use tokio::sync::RwLock;

use twilight_model::id::{Id, marker::{ChannelMarker, GuildMarker, UserMarker}};
use twilight_model::gateway::payload::incoming::{VoiceStateUpdate, VoiceServerUpdate};
use twilight_model::gateway::payload::outgoing::UpdateVoiceState;
use twilight_gateway::Cluster;

/// [`Player`] manager across many guilds.
///
/// This should be made for each shard or cluster.
#[derive(Clone)]
pub struct Manager {
    mref: Arc<ManagerRef>,
    me: Id<UserMarker>,
    gateway: Arc<Cluster>,
}

struct ManagerRef {
    players: RwLock<HashMap<Id<GuildMarker>, Player>>,
}

impl Manager {
    /// Creates a new manager.
    pub fn new(gateway: Arc<Cluster>, me: Id<UserMarker>) -> Manager {
        Manager {
            mref: Arc::new(ManagerRef {
                players: RwLock::new(HashMap::new()),
            }),
            me, gateway,
        }
    }

    /// Gets the player currently playing in a voice guild.
    ///
    /// Returns `None` if it does not exist or the player is closed.
    pub async fn get(
        &self,
        guild_id: impl Into<Id<GuildMarker>>,
    ) -> Option<Player> {
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
        self.gateway.command(0, &UpdateVoiceState::new(guild_id, channel_id, false, false)).await.unwrap();

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
    async fn new_player(
        &self,
        guild_id: Id<GuildMarker>,
    ) -> Player {
        let player = Player::new(self.me, guild_id).await;

        self.mref.players
            .write()
            .await
            .insert(guild_id, player.clone());

        player
    }

    /// Gets a player.
    ///
    /// Returns `None` if it does not exist or the player is closed.
    async fn get_player(&self, id: Id<GuildMarker>) -> Option<Player> {
        self.mref.players
            .read()
            .await
            .get(&id)
            .and_then(|p| if p.is_closed() { None } else { Some(p.clone()) })
    }
}

