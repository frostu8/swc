//! Audio player commands.

use super::audio::Track;

use twilight_model::{
    application::interaction::Interaction as DiscordInteraction,
    id::{Id, marker::InteractionMarker},
};

/// A command given to an audio player.
pub struct Command {
    /// The interaction associated with the command.
    pub interaction: Interaction,
    /// The action to perform.
    pub kind: CommandType,
}

/// The interaction associated with the command.
pub struct Interaction {
    /// The id of the interaction.
    pub id: Id<InteractionMarker>,
    /// The token of the interaction.
    pub token: String,
}

impl From<DiscordInteraction> for Interaction {
    fn from(d: DiscordInteraction) -> Interaction {
        Interaction {
            id: d.id,
            token: d.token,
        }
    }
}

/// The specific kind of command to handle.
pub enum CommandType {
    /// Pushes a new track onto the queue.
    Play(Track),
}
