//! Commands that a user can input for a music queue.

use std::fmt::Display;

use twilight_http::{
    client::{Client as HttpClient, InteractionClient},
    response::{Response, marker::EmptyBody},
    Error as HttpError,
};
use twilight_model::{
    channel::message::MessageFlags,
    http::interaction::{
        InteractionResponse, InteractionResponseType, InteractionResponseData,
    },
    id::{Id, marker::{ApplicationMarker, GuildMarker, InteractionMarker, UserMarker}},
};

/// A single command.
///
/// Holds information about the command and how to respond to it.
pub struct Command {
    pub interaction_id: Id<InteractionMarker>,
    pub interaction_token: String,

    pub application_id: Id<ApplicationMarker>,
    pub guild_id: Id<GuildMarker>,
    pub user_id: Id<UserMarker>,

    pub action: Action,
}

/// The action that a commands wants completed.
pub enum Action {
    /// Plays a track, with a URL to query YTDL with.
    Play(String),
    /// Skips the currently playing track.
    Skip,
}

impl Command {
    /// Begins a command response.
    pub fn respond<'a>(&'a self, client: &'a HttpClient) -> CommandResponse<'a> {
        CommandResponse {
            command: self,
            client: client.interaction(self.application_id),
        }
    }
}

/// A builder for a response to a command.
pub struct CommandResponse<'a> {
    command: &'a Command,
    client: InteractionClient<'a>,
}

impl<'a> CommandResponse<'a> {
    /// Responds with a quick, user-friendly error.
    pub async fn error(&self, error: impl Display) -> Result<Response<EmptyBody>, HttpError> {
        let error = error.to_string();

        self
            .client
            .create_response(
                self.command.interaction_id,
                &self.command.interaction_token,
                &InteractionResponse {
                    kind: InteractionResponseType::ChannelMessageWithSource,
                    data: Some(InteractionResponseData {
                        flags: Some(MessageFlags::EPHEMERAL),
                        content: Some(error),
                        ..Default::default()
                    }),
                },
            )
            .await
    }
}

