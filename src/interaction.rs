//! Interaction utilities.

use twilight_model::{
    http::interaction::{InteractionResponse, InteractionResponseType, InteractionResponseData},
    channel::message::{Embed, MessageFlags},
    id::{Id, marker::{InteractionMarker}},
};

use twilight_http::{
    request::application::interaction::CreateResponse, client::InteractionClient,
    response::{ResponseFuture, marker::EmptyBody},
    Error,
};

use std::fmt::Display;
use std::future::IntoFuture;

/// Extension methods for Twilight's interaction client.
pub trait ResponseExt {
    /// Creates a [`Response`] for an interaction.
    fn respond<'a>(&'a self, id: Id<InteractionMarker>, token: &'a str) -> Response<'a>;

    /// Acknowledges an interaction.
    fn ack(&self, id: Id<InteractionMarker>, token: &str) -> ResponseFuture<EmptyBody>;
}

impl<'c> ResponseExt for InteractionClient<'c> {
    fn respond<'a>(
        &'a self,
        id: Id<InteractionMarker>,
        token: &'a str,
    ) -> Response<'a> {
        Response {
            client: self,
            id,
            token,

            data: Default::default(),
        }
    }

    fn ack(
        &self,
        id: Id<InteractionMarker>,
        token: &str,
    ) -> ResponseFuture<EmptyBody> {
        self
            .create_response(
                id,
                &token,
                &InteractionResponse {
                    kind: InteractionResponseType::DeferredChannelMessageWithSource,
                    data: None,
                },
            )
            .into_future()
    }
}

/// A response builder for an interaction.
///
/// Provides shorthands for commonly used types.
pub struct Response<'a> {
    client: &'a InteractionClient<'a>,
    id: Id<InteractionMarker>,
    token: &'a str,

    data: InteractionResponseData,
}

impl<'a> Response<'a> {
    /// Responds with some content.
    pub fn content<T>(self, content: T) -> Response<'a>
    where
        T: ToString
    {
        Response {
            client: self.client,
            id: self.id,
            token: self.token,
            data: InteractionResponseData {
                content: Some(content.to_string()),
                ..self.data
            },
        }
    }

    /// Responds with an embed.
    pub fn embed(self, embed: Embed) -> Response<'a> {
        Response {
            client: self.client,
            id: self.id,
            token: self.token,
            data: InteractionResponseData {
                embeds: Some(vec![embed]),
                ..self.data
            },
        }
    }

    /// Marks the message as "ephemeral."
    pub fn ephemeral(self) -> Response<'a> {
        Response {
            client: self.client,
            id: self.id,
            token: self.token,
            data: InteractionResponseData {
                flags: Some(MessageFlags::EPHEMERAL),
                ..self.data
            },
        }
    }

    /// Responds with a standardized error error message.
    pub fn error<T>(self, err: T) -> Response<'a>
    where
        T: Display
    {
        self
            .content(format!("error:\n{}", err))
            .ephemeral()
    }

    /// Updates an already sent response.
    pub async fn update(self) -> Result<(), Error> {
        self
            .client
            .update_response(&self.token)
            .content(self.data.content.as_ref().map(|x| &**x))
            .unwrap()
            .embeds(self.data.embeds.as_ref().map(|x| &x[..]))
            .unwrap()
            .await
            .map(|_| ())
    }
}

impl<'a> IntoFuture for Response<'a> {
    type Output = <CreateResponse<'a> as IntoFuture>::Output;
    type IntoFuture = <CreateResponse<'a> as IntoFuture>::IntoFuture;

    fn into_future(self) -> Self::IntoFuture {
        self
            .client
            .create_response(
                self.id,
                self.token,
                &InteractionResponse {
                    kind: InteractionResponseType::ChannelMessageWithSource,
                    data: Some(self.data),
                },
            )
            .into_future()
    }
}
