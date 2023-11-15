//! Commands that a user can input for a music queue.

use std::fmt::Display;
use std::ops::Deref;

use twilight_http::{
    client::{Client as HttpClient, InteractionClient},
    response::{Response, marker::EmptyBody},
    Error as HttpError,
};
use twilight_model::{
    channel::{message::{MessageFlags, Embed}, Message},
    http::interaction::{
        InteractionResponse, InteractionResponseType, InteractionResponseData,
    },
    id::{Id, marker::{ApplicationMarker, GuildMarker, InteractionMarker, UserMarker}},
};

/// A single command.
///
/// Holds information about the command and how to respond to it.
pub struct Command {
    pub data: CommandData,
    pub action: Action,
}

/// The actual command data.
#[derive(Clone)]
pub struct CommandData {
    pub interaction_id: Id<InteractionMarker>,
    pub interaction_token: String,

    pub application_id: Id<ApplicationMarker>,
    pub guild_id: Id<GuildMarker>,
    pub user_id: Id<UserMarker>,
}

/// The action that a commands wants completed.
pub enum Action {
    /// Plays a track, with a URL to query YTDL with.
    Play(String),
    /// Skips the currently playing track.
    Skip,
    /// Lists all of the tracks in a queue.
    Queue,
    /// Shuffles the tracks in a queue.
    Shuffle,
}

impl CommandData {
    /// Begins a command response.
    pub fn respond<'a>(&'a self, client: &'a HttpClient) -> CommandResponse<'a> {
        CommandResponse {
            command: self,
            client: client.interaction(self.application_id),

            content: None,
            embeds: None,
            flags: MessageFlags::empty(),
        }
    }
}

impl Deref for Command {
    type Target = CommandData;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

/// A builder for a response to a command.
pub struct CommandResponse<'a> {
    command: &'a CommandData,
    client: InteractionClient<'a>,

    content: Option<String>,
    embeds: Option<Vec<Embed>>,
    flags: MessageFlags,
}

impl<'a> CommandResponse<'a> {
    /// Sets the response as a quick, user friendly error.
    pub fn error(&mut self, error: impl Display) -> &mut Self {
        self.content = Some(error.to_string());
        self.flags |= MessageFlags::EPHEMERAL;

        self
    }

    /// Sets the content of the message.
    pub fn content(&mut self, content: impl Display) -> &mut Self {
        self.content = Some(content.to_string());

        self
    }

    /// Adds an embed to the response.
    pub fn embed(&mut self, embed: Embed) -> &mut Self {
        if self.embeds.is_none() {
            self.embeds = Some(Vec::new());
        }

        self.embeds.as_mut().unwrap().push(embed);

        self
    }

    /// Acks the response.
    /// 
    /// The final message must be updated with [`CommandResponse::update`].
    pub async fn ack(&mut self) -> Result<Response<EmptyBody>, HttpError> {
        self
            .client
            .create_response(
                self.command.interaction_id,
                &self.command.interaction_token,
                &InteractionResponse {
                    kind: InteractionResponseType::DeferredChannelMessageWithSource,
                    data: None,
                },
            )
            .await
    }

    /// Updates the previous message (mostly an ACK).
    pub async fn update(&mut self) -> Result<Response<Message>, HttpError> {
        self
            .client
            .update_response(&self.command.interaction_token)
            .content(self.content.as_deref())
            .unwrap()
            .embeds(self.embeds.as_deref())
            .unwrap()
            .await
    }

    /// Responds with a new message.
    pub async fn respond(&mut self) -> Result<Response<EmptyBody>, HttpError> {
        self
            .client
            .create_response(
                self.command.interaction_id,
                &self.command.interaction_token,
                &InteractionResponse {
                    kind: InteractionResponseType::ChannelMessageWithSource,
                    data: Some(InteractionResponseData {
                        flags: Some(self.flags),
                        embeds: self.embeds.take(),
                        content: self.content.take(),
                        ..Default::default()
                    }),
                },
            )
            .await
    }
}

