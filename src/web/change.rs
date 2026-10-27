//! Requests that change something. Each is a form `POST` that must come
//! from this site and carry the viewer's form token ([`Gate::form_token`]),
//! which every page puts in its forms. A handler that takes [`Change`] gets
//! both checks; there is no other way to read a changing form.

use super::sign_in::{same_origin, session_cookie};
use super::source::Gate;
use axum::Form;
use axum::extract::{FromRef, FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::sync::Arc;

/// A changing form's fields, `T`, read only once the request has passed the
/// checks. `T`'s fields are text, as form fields are.
pub struct Change<T>(pub T);

/// A form as sent: the token beside the fields.
#[derive(Deserialize)]
struct Sent<T> {
    #[serde(default)]
    form_token: String,
    #[serde(flatten)]
    fields: T,
}

impl<S, T> FromRequest<S> for Change<T>
where
    S: Send + Sync,
    Arc<dyn Gate>: FromRef<S>,
    T: DeserializeOwned + Send,
{
    type Rejection = Response;

    async fn from_request(request: Request, state: &S) -> Result<Self, Response> {
        let gate = Arc::<dyn Gate>::from_ref(state);
        if !same_origin(request.headers()) {
            return Err(refused("the form was sent from another site"));
        }
        let session = session_cookie(request.headers());
        let Form(sent) = Form::<Sent<T>>::from_request(request, state)
            .await
            .map_err(IntoResponse::into_response)?;
        if !gate.form_token_matches(session.as_deref(), &sent.form_token) {
            return Err(refused(
                "the form did not come from this site's page; reload the page and try again",
            ));
        }
        Ok(Change(sent.fields))
    }
}

/// A form with no fields but the token.
#[derive(Deserialize)]
pub struct NoFields {}

fn refused(reason: &str) -> Response {
    (StatusCode::FORBIDDEN, format!("refused: {reason}")).into_response()
}
