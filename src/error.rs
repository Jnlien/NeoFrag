use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};

/// Erreur technique d'un handler. Les « 404 » et « 403 » métier sont rendus dans le
/// thème par `layout::not_found` / `layout::forbidden` ; ici on ne traite que l'imprévu.
#[derive(Debug)]
pub enum AppError {
    Db(sqlx::Error),
    Template(askama::Error),
    BadRequest(&'static str),
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

impl From<askama::Error> for AppError {
    fn from(e: askama::Error) -> Self {
        Self::Template(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            AppError::Db(e) => {
                tracing::error!(error = %e, "erreur base de données");
                (StatusCode::INTERNAL_SERVER_ERROR, "Erreur interne du serveur")
            }
            AppError::Template(e) => {
                tracing::error!(error = %e, "erreur de rendu");
                (StatusCode::INTERNAL_SERVER_ERROR, "Erreur interne du serveur")
            }
        };
        let body = format!(
            "<!DOCTYPE html><html lang=\"fr\"><meta charset=\"utf-8\"><title>{code}</title>\
             <body style=\"font-family:sans-serif;text-align:center;padding:4rem\">\
             <h1>{code}</h1><p>{message}</p></body></html>",
            code = status.as_u16()
        );
        (status, Html(body)).into_response()
    }
}
