//! Audio player commands.

use super::audio::Track;

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
    /// Whether to update an already sent acknowledgement or create an entirely
    /// new response.
    pub update: bool,
    /// The action to perform.
    pub kind: CommandType,
}

impl Command {
    /// Responds to the command.
    pub fn respond<'a>(&'a self) -> CommandResponse<'a> {
        CommandResponse::new(&self.interaction).update(self.update)
    }
}

/// A response to a command.
pub struct CommandResponse<'a> {
    interaction: &'a Interaction,
    update: bool,

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

        if self.update {
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
            update: false,

            content: None,
            embed: None,
        }
    }

    fn update(self, update: bool) -> CommandResponse<'a> {
        CommandResponse {
            update,
            ..self
        }
    }
}

/// The interaction associated with the command.
pub struct Interaction {
    /// The id of the interaction.
    pub id: Id<InteractionMarker>,
    /// The token of the interaction.
    pub token: String,
    application_id: Id<ApplicationMarker>,
}

impl From<DiscordInteraction> for Interaction {
    fn from(d: DiscordInteraction) -> Interaction {
        Interaction {
            id: d.id,
            token: d.token,
            application_id: d.application_id,
        }
    }
}

/// The specific kind of command to handle.
pub enum CommandType {
    /// Pushes a new track onto the queue.
    Play(Track),
    /// Skips the currently playing track.
    Skip,
}
