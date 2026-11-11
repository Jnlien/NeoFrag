//! Liens envoyés par email : validation de l'inscription et mot de passe oublié.
//! Porté de `modules/user` (actions `lost_password`, `_lost_password` et validation).
//!
//! Comme dans l'original, un compte « à valider » est un compte sans date de dernière
//! activité quand le réglage `nf_registration_validation` est actif.
//!
//! Différences volontaires : la demande de réinitialisation répond toujours la même
//! chose (l'original révèle si une adresse est inscrite), les liens expirent, et un
//! nouveau mot de passe déconnecte toutes les sessions du compte.

use super::{
    account::{validate_email, validate_new_password},
    user::{ICON, TITLE},
};
use crate::{
    core::{
        ctx::Ctx,
        form,
        layout::{self, Page},
        mail::{self, Draft},
        panel::Panel,
        password, session, text, throttle,
        token::{self, Kind},
    },
    error::AppError,
};
use axum::{
    extract::Path,
    http::header,
    response::{IntoResponse, Redirect, Response},
    Extension, Form,
};
use serde::Deserialize;
use sqlx::Row;
use std::{sync::Arc, time::Duration};

type Ctxt = Extension<Arc<Ctx>>;
type Reply = Result<Response, AppError>;

const HOUR: Duration = Duration::from_secs(3600);
const LOST_TITLE: &str = "Mot de passe oublié";
const LOST_ICON: &str = "fas fa-unlock-alt";
const SESSION_EXPIRED: &str = "Votre session a expiré, veuillez réessayer.";

/// Le site exige-t-il que les nouveaux inscrits valident leur adresse email ?
pub fn validation_required(ctx: &Ctx) -> bool {
    ctx.settings.bool("nf_registration_validation")
}

/// Page étroite centrée, avec un seul panel.
async fn narrow_page(ctx: &Ctx, title: &'static str, path: &str, panel: Panel) -> Reply {
    let page = Page::new("user", TITLE, ICON)
        .title(title)
        .crumb(title, path.to_owned())
        .content(format!(
            "<div class=\"row justify-content-center\"><div class=\"col-12 col-lg-6\">{}</div></div>",
            panel.render()
        ));
    layout::render(ctx, page).await
}

/// Page d'information (message déjà en HTML sûr, fourni par le code).
pub async fn notice_page(ctx: &Ctx, title: &'static str, icon: &str, path: &str, message: &str) -> Reply {
    let panel = Panel::new()
        .heading(title, icon)
        .body(format!("<div class=\"text-center\">{message}</div>"));
    narrow_page(ctx, title, path, panel).await
}

// ---------------------------------------------------------------------------
// Validation de l'inscription

/// Envoie (ou renvoie) le lien de validation. `false` si l'email n'a pas pu partir.
pub async fn send_validation(ctx: &Ctx, user_id: u32, username: &str, email: &str) -> Result<bool, AppError> {
    let token = token::create(ctx, user_id, Kind::Validation).await?;
    let paragraphs = [
        format!("Bonjour {username},"),
        "Afin de valider votre inscription sur notre site web, merci de bien vouloir cliquer sur le bouton ci-dessous.".to_owned(),
        "Ce lien est valable 7 jours. Si vous n'êtes pas à l'origine de cette inscription, ignorez ce message.".to_owned(),
    ];
    let draft = Draft {
        to: email,
        reply_to: None,
        subject: "Validation de votre compte",
        paragraphs: &paragraphs,
        button: Some(("Valider mon compte", &format!("user/validation/{token}"))),
    };
    match mail::build(ctx, draft) {
        Ok(email) => Ok(email.send().await),
        Err(_) => Ok(false),
    }
}

/// À la connexion d'un compte non validé : renvoie le lien, au plus une fois par quart d'heure.
pub async fn resend_validation(ctx: &Ctx, user_id: u32, username: &str, email: &str) -> Result<(), AppError> {
    let key = format!("validation:{user_id}");
    let window = Duration::from_secs(15 * 60);
    if email.is_empty() || !mail::enabled(ctx) || throttle::blocked(&ctx.state, &key, 1, window) {
        return Ok(());
    }
    throttle::hit(&ctx.state, &key, window);
    send_validation(ctx, user_id, username, email).await?;
    Ok(())
}

/// `user/validation/{jeton}` : valide le compte et connecte son propriétaire.
pub async fn validation(Extension(ctx): Ctxt, Path(token): Path<String>) -> Reply {
    let Some(user_id) = token::user_of(&ctx, &token, Kind::Validation).await? else {
        let message = format!(
            "Ce lien n'est pas valide ou a expiré.<br />Si votre compte est déjà validé, vous pouvez <a href=\"{}\">vous connecter</a>.",
            ctx.url("user/login")
        );
        return notice_page(&ctx, "Validation de votre compte", "fas fa-user-check", "user/login", &message).await;
    };
    sqlx::query("UPDATE nf_user SET last_activity_date = NOW() WHERE id = ?")
        .bind(user_id)
        .execute(&ctx.state.db)
        .await?;
    token::revoke_all(&ctx, user_id).await?;

    if ctx.session_id.is_empty() || ctx.user.is_some() {
        // Robot (pré-chargement du lien par une messagerie) ou visiteur déjà connecté.
        return Ok(Redirect::to(&ctx.url("user/login")).into_response());
    }
    let cookie = session::login(&ctx, user_id, false).await?;
    Ok(([(header::SET_COOKIE, cookie.to_string())], Redirect::to(&ctx.url("user/edit?ok=welcome"))).into_response())
}

// ---------------------------------------------------------------------------
// Mot de passe oublié

#[derive(Deserialize, Default)]
pub struct LostForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    email: String,
}

async fn lost_page(ctx: &Ctx, email: &str, error: Option<&str>) -> Reply {
    if !mail::enabled(ctx) {
        return notice_page(
            ctx,
            LOST_TITLE,
            LOST_ICON,
            "user/lost-password",
            "La réinitialisation par email n'est pas disponible sur ce site. Contactez un administrateur.",
        )
        .await;
    }
    let fields = format!(
        "{}<p>Indiquez l'adresse email de votre compte : vous recevrez un lien pour choisir un nouveau mot de passe.</p>{}",
        form::alert("danger", error),
        form::email("Adresse email", "email", email, true),
    );
    let body = format!(
        "<form method=\"post\" action=\"{}\">{}{fields}<button type=\"submit\" class=\"btn btn-primary\">Envoyer</button> \
         <a class=\"btn btn-link\" href=\"{}\">Annuler</a></form>",
        ctx.url("user/lost-password"),
        form::csrf_field(ctx),
        ctx.url("user/login"),
    );
    narrow_page(ctx, LOST_TITLE, "user/lost-password", Panel::new().heading(LOST_TITLE, LOST_ICON).body(body)).await
}

pub async fn lost_get(Extension(ctx): Ctxt) -> Reply {
    if ctx.user.is_some() {
        return Ok(Redirect::to(&ctx.url("user/account")).into_response());
    }
    lost_page(&ctx, "", None).await
}

pub async fn lost_post(Extension(ctx): Ctxt, Form(form): Form<LostForm>) -> Reply {
    if ctx.user.is_some() {
        return Ok(Redirect::to(&ctx.url("user/account")).into_response());
    }
    let email = form.email.trim();
    if !mail::enabled(&ctx) {
        return lost_page(&ctx, email, None).await;
    }
    if !ctx.csrf_ok(&form.csrf) {
        return lost_page(&ctx, email, Some(SESSION_EXPIRED)).await;
    }
    if let Err(message) = validate_email(email) {
        return lost_page(&ctx, email, Some(message)).await;
    }
    let ip_key = format!("lost:{}", ctx.ip);
    if throttle::blocked(&ctx.state, &ip_key, 5, HOUR) {
        return lost_page(&ctx, email, Some("Trop de demandes, réessayez plus tard.")).await;
    }
    throttle::hit(&ctx.state, &ip_key, HOUR);

    let row = sqlx::query("SELECT id, username FROM nf_user WHERE email = ? AND deleted = '0' LIMIT 1")
        .bind(email)
        .fetch_optional(&ctx.state.db)
        .await?;
    if let Some(row) = row {
        let user_id: u32 = row.try_get("id")?;
        let username = text::decode(&row.try_get::<String, _>("username")?);
        let user_key = format!("lost-user:{user_id}");
        if !throttle::blocked(&ctx.state, &user_key, 3, HOUR) {
            throttle::hit(&ctx.state, &user_key, HOUR);
            let token = token::create(&ctx, user_id, Kind::Password).await?;
            let paragraphs = [
                format!("Bonjour {username},"),
                "Vous avez demandé à réinitialiser votre mot de passe. Il vous suffit de cliquer sur le bouton ci-dessous pour choisir un nouveau mot de passe.".to_owned(),
                "Ce lien est valable 2 heures. Si vous n'êtes pas à l'origine de cette demande, ignorez ce message : votre mot de passe reste inchangé.".to_owned(),
            ];
            let draft = Draft {
                to: email,
                reply_to: None,
                subject: "Réinitialisation de mot de passe",
                paragraphs: &paragraphs,
                button: Some(("Réinitialiser mon mot de passe", &format!("user/lost-password/{token}"))),
            };
            if let Ok(email) = mail::build(&ctx, draft) {
                // En tâche de fond : le temps de réponse ne doit pas révéler si l'adresse est inscrite.
                tokio::spawn(email.send());
            }
        }
    }

    notice_page(
        &ctx,
        LOST_TITLE,
        LOST_ICON,
        "user/lost-password",
        "Si un compte est associé à cette adresse, un email contenant un lien de réinitialisation vient de lui être envoyé.",
    )
    .await
}

#[derive(Deserialize, Default)]
pub struct ResetForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    password_confirm: String,
}

async fn invalid_link(ctx: &Ctx) -> Reply {
    let message = format!(
        "Ce lien n'est pas valide ou a expiré. Vous pouvez <a href=\"{}\">refaire une demande</a>.",
        ctx.url("user/lost-password")
    );
    notice_page(ctx, LOST_TITLE, LOST_ICON, "user/lost-password", &message).await
}

async fn reset_page(ctx: &Ctx, token: &str, error: Option<&str>) -> Reply {
    let fields = format!(
        "{}{}{}",
        form::alert("danger", error),
        form::password("Nouveau mot de passe", "password", "new-password", true, "8 caractères minimum."),
        form::password("Confirmation", "password_confirm", "new-password", true, ""),
    );
    // `token` a été vérifié : 32 caractères [a-z0-9].
    let body = format!(
        "<form method=\"post\" action=\"{}\">{}{fields}<button type=\"submit\" class=\"btn btn-primary\">Enregistrer</button></form>",
        ctx.url(&format!("user/lost-password/{token}")),
        form::csrf_field(ctx),
    );
    let title = "Réinitialisation de mot de passe";
    narrow_page(ctx, title, "user/lost-password", Panel::new().heading(title, LOST_ICON).body(body)).await
}

pub async fn reset_get(Extension(ctx): Ctxt, Path(token): Path<String>) -> Reply {
    if ctx.user.is_some() {
        return Ok(Redirect::to(&ctx.url("user/account")).into_response());
    }
    if token::user_of(&ctx, &token, Kind::Password).await?.is_none() {
        return invalid_link(&ctx).await;
    }
    reset_page(&ctx, &token, None).await
}

pub async fn reset_post(Extension(ctx): Ctxt, Path(token): Path<String>, Form(form): Form<ResetForm>) -> Reply {
    if ctx.user.is_some() {
        return Ok(Redirect::to(&ctx.url("user/account")).into_response());
    }
    let Some(user_id) = token::user_of(&ctx, &token, Kind::Password).await? else {
        return invalid_link(&ctx).await;
    };
    if !ctx.csrf_ok(&form.csrf) {
        return reset_page(&ctx, &token, Some(SESSION_EXPIRED)).await;
    }
    if let Err(message) = validate_new_password(&form.password, &form.password_confirm) {
        return reset_page(&ctx, &token, Some(message)).await;
    }

    let (hash, salt) = password::hash(&ctx.state.config.security.password_salt, &form.password);
    let db = &ctx.state.db;
    // Le lien prouve la possession de l'adresse : le compte est validé du même coup.
    sqlx::query("UPDATE nf_user SET password = ?, salt = ?, last_activity_date = NOW() WHERE id = ?")
        .bind(hash)
        .bind(salt)
        .bind(user_id)
        .execute(db)
        .await?;
    token::revoke_all(&ctx, user_id).await?;
    sqlx::query("UPDATE nf_session SET user_id = NULL, remember = '0' WHERE user_id = ?")
        .bind(user_id)
        .execute(db)
        .await?;

    let cookie = session::login(&ctx, user_id, false).await?;
    Ok(([(header::SET_COOKIE, cookie.to_string())], Redirect::to(&ctx.url("user/account?ok=reset"))).into_response())
}
