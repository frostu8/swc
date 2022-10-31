//! Audio player commands.

use super::audio::{Playlist, Track};

use std::sync::Arc;

use twilight_model::{
    application::interaction::Interaction as DiscordInteraction,
    http::interaction::{InteractionResponseData, InteractionResponse, InteractionResponseType},
    channel::embed::Embed,
    id::{Id, marker::{ApplicationMarker, InteractionMarker}},
};
use twilight_http::{error::Error, client::Client};

/// A command given to an audio player.
pub struct Command {
    /// The interaction associated with the command.
    pub interaction: Interaction,
    /// The action to perform.
    pub kind: CommandType,
}

impl Command {
    /// Creates a new `Command`.
    pub fn new(
        interaction: impl Into<Interaction>,
        kind: CommandType,
        update: bool,
    ) -> Command {
        Command {
            interaction: Interaction {
                update,
                ..interaction.into()
            },
            kind,
        }
    }
}

/// The interaction associated with the command.
pub struct Interaction {
    /// The id of the interaction.
    pub id: Id<InteractionMarker>,
    /// The token of the interaction.
    pub token: String,
    /// Whether to update an already sent acknowledgement or create an entirely
    /// new response.
    pub update: bool,
    application_id: Id<ApplicationMarker>,
}

impl Interaction {
    /// Responds to the interaction.
    pub fn respond<'a>(&'a self) -> CommandResponse<'a> {
        CommandResponse::new(&self)
    }
}

/// A response to a command.
pub struct CommandResponse<'a> {
    interaction: &'a Interaction,

    content: Option<String>,
    embed: Option<Embed>,
}

impl<'a> CommandResponse<'a> {
    /// Attaches some content to the response.
    pub fn content(self, content: impl ToString) -> CommandResponse<'a> {
        CommandResponse {
            content: Some(content.to_string()),
            ..self
        }
    }

    /// Attaches an embed to the response.
    pub fn embed(self, embed: Embed) -> CommandResponse<'a> {
        CommandResponse {
            embed: Some(embed),
            ..self
        }
    }

    /// Sends the response.
    pub async fn send<'c>(
        self,
        http_client: Arc<Client>,
    ) -> Result<(), Error> {
        let client = http_client.interaction(self.interaction.application_id);

        if self.interaction.update {
            client
                .update_response(&self.interaction.token)
                .content(self.content.as_ref().map(|x| &**x))
                .unwrap()
                .embeds(self.embed.as_ref().map(|e| std::slice::from_ref(e)))
                .unwrap()
                .exec()
                .await
                .map(|_| ())
        } else {
            client
                .create_response(
                    self.interaction.id,
                    &self.interaction.token,
                    &InteractionResponse {
                        kind: InteractionResponseType::ChannelMessageWithSource,
                        data: Some(InteractionResponseData {
                            content: self.content,
                            embeds: self.embed.map(|e| vec![e]),
                            ..Default::default()
                        }),
                    },
                )
                .exec()
                .await
                .map(|_| ())
        }
    }

    fn new(interaction: &'a Interaction) -> CommandResponse<'a> {
        CommandResponse {
            interaction,

            content: None,
            embed: None,
        }
    }
}

impl From<DiscordInteraction> for Interaction {
    fn from(d: DiscordInteraction) -> Interaction {
        Interaction {
            id: d.id,
            token: d.token,
            update: false,
            application_id: d.application_id,
        }
    }
}

/// The specific kind of command to handle.
pub enum CommandType {
    /// Pushes a new track onto the queue.
    Play(Track),
    /// Pushes the head of a new playlist onto the queue.
    PlayList(Playlist),
    /// Skips the currently playing track.
    Skip,
}
