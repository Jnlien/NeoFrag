//! Module Utilisateur : connexion, déconnexion, espace membre et profils publics.
//! Porté de `modules/user` (inscription, messagerie et édition du profil : voir ROADMAP).

use crate::{
    core::{
        ctx::Ctx,
        layout::{self, Page},
        panel::{Block, Panel},
        password, session, text, user,
    },
    error::AppError,
    state::AppState,
};
use axum::{
    extract::Path,
    http::header,
    response::{IntoResponse, Redirect, Response},
    Extension, Form,
};
use chrono::NaiveDateTime;
use serde::Deserialize;
use sqlx::Row;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub const TITLE: &str = "Espace membre";
pub const ICON: &str = "fas fa-user";

const MAX_FAILED_LOGINS: u32 = 8;
const LOCKOUT: Duration = Duration::from_secs(15 * 60);

/// Formulaire de connexion (page dédiée et widget « Espace membre »).
pub fn login_form(ctx: &Ctx, error: Option<&str>) -> String {
    let alert = match error {
        Some(message) => format!("<div class=\"alert alert-danger\">{message}</div>"),
        None => String::new(),
    };
    format!(
        "{alert}<form method=\"post\" action=\"{action}\">\
         <input type=\"hidden\" name=\"csrf\" value=\"{csrf}\" />\
         <div class=\"form-group\"><input type=\"text\" class=\"form-control\" name=\"login\" placeholder=\"Pseudo ou adresse email\" autocomplete=\"username\" required /></div>\
         <div class=\"form-group\"><input type=\"password\" class=\"form-control\" name=\"password\" placeholder=\"Mot de passe\" autocomplete=\"current-password\" required /></div>\
         <div class=\"form-group form-check\"><input type=\"checkbox\" class=\"form-check-input\" name=\"remember\" value=\"on\" id=\"login-remember\" />\
         <label class=\"form-check-label\" for=\"login-remember\">Se souvenir de moi</label></div>\
         <button type=\"submit\" class=\"btn btn-primary\">Se connecter</button></form>",
        action = ctx.url("user/login"),
        csrf = ctx.csrf_token(),
    )
}

async fn login_page(ctx: &Ctx, error: Option<&str>) -> Result<Response, AppError> {
    let panel = Panel::new()
        .heading("Connexion", "fas fa-sign-in-alt")
        .body(login_form(ctx, error));
    let page = Page::new("user", TITLE, ICON)
        .title("Connexion")
        .crumb("Connexion", "user/login")
        .content(format!(
            "<div class=\"row justify-content-center\"><div class=\"col-12 col-lg-6\">{}</div></div>",
            panel.render()
        ));
    layout::render(ctx, page).await
}

pub async fn login_get(Extension(ctx): Extension<Arc<Ctx>>) -> Result<Response, AppError> {
    if ctx.user.is_some() {
        return Ok(Redirect::to(&ctx.url("user")).into_response());
    }
    login_page(&ctx, None).await
}

#[derive(Deserialize)]
pub struct LoginForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    login: String,
    #[serde(default)]
    password: String,
    remember: Option<String>,
}

/// Trop de tentatives ratées récentes depuis cette adresse ?
fn locked_out(state: &AppState, ip: &str) -> bool {
    let attempts = state.login_attempts.lock().unwrap_or_else(|e| e.into_inner());
    attempts
        .get(ip)
        .is_some_and(|(count, since)| *count >= MAX_FAILED_LOGINS && since.elapsed() < LOCKOUT)
}

fn record_failure(state: &AppState, ip: &str) {
    let mut attempts = state.login_attempts.lock().unwrap_or_else(|e| e.into_inner());
    attempts.retain(|_, (_, since)| since.elapsed() < LOCKOUT);
    let entry = attempts.entry(ip.to_owned()).or_insert((0, Instant::now()));
    entry.0 += 1;
}

pub async fn login_post(
    Extension(ctx): Extension<Arc<Ctx>>,
    Form(form): Form<LoginForm>,
) -> Result<Response, AppError> {
    if !ctx.csrf_ok(&form.csrf) {
        return login_page(&ctx, Some("Votre session a expiré, veuillez réessayer.")).await;
    }
    let state = &ctx.state;
    if locked_out(state, &ctx.ip) {
        return login_page(&ctx, Some("Trop de tentatives, veuillez patienter quelques minutes.")).await;
    }

    let row = sqlx::query(
        "SELECT id, password, salt FROM nf_user WHERE deleted = '0' AND (username = ? OR email = ?) LIMIT 1",
    )
    .bind(&form.login)
    .bind(&form.login)
    .fetch_optional(&state.db)
    .await?;

    let global_salt = &state.config.security.password_salt;
    let verified = match &row {
        Some(row) => {
            let stored: String = row.try_get("password")?;
            let salt: String = row.try_get("salt")?;
            password::verify(global_salt, &form.password, &stored, &salt)
                .map(|v| Ok::<_, sqlx::Error>((row.try_get::<u32, _>("id")?, v)))
                .transpose()?
        }
        None => None,
    };

    let Some((user_id, verified)) = verified else {
        record_failure(state, &ctx.ip);
        return login_page(&ctx, Some("Identifiants invalides")).await;
    };

    if verified.needs_rehash {
        // Compte d'avant les sels (ex. « admin » de l'installation) : on le met à niveau.
        let (hash, salt) = password::hash(global_salt, &form.password);
        sqlx::query("UPDATE nf_user SET password = ?, salt = ? WHERE id = ?")
            .bind(hash)
            .bind(salt)
            .bind(user_id)
            .execute(&state.db)
            .await?;
    }

    let cookie = session::login(&ctx, user_id, form.remember.as_deref() == Some("on")).await?;
    Ok(([(header::SET_COOKIE, cookie.to_string())], Redirect::to(&ctx.url("user"))).into_response())
}

#[derive(Deserialize)]
pub struct CsrfForm {
    #[serde(default)]
    pub csrf: String,
}

pub async fn logout(Extension(ctx): Extension<Arc<Ctx>>, Form(form): Form<CsrfForm>) -> Result<Response, AppError> {
    if ctx.csrf_ok(&form.csrf) {
        session::logout(&ctx).await?;
    }
    Ok(Redirect::to(&ctx.url("")).into_response())
}

struct Profile {
    id: u32,
    username: String,
    admin: bool,
    registration_date: NaiveDateTime,
    last_activity_date: Option<NaiveDateTime>,
    avatar: Option<String>,
    sex: Option<String>,
    quote: String,
    location: String,
    website: String,
    signature: String,
    online: bool,
}

async fn load_profile(ctx: &Ctx, user_id: u32) -> Result<Option<Profile>, AppError> {
    let row = sqlx::query(
        "SELECT u.id, u.username, (u.admin = '1') AS admin, CAST(u.registration_date AS DATETIME) AS registration_date, \
                CAST(u.last_activity_date AS DATETIME) AS last_activity_date, \
                f.path AS avatar, up.sex, IFNULL(up.quote, '') AS quote, IFNULL(up.location, '') AS location, \
                IFNULL(up.website, '') AS website, IFNULL(up.signature, '') AS signature, \
                EXISTS(SELECT 1 FROM nf_session s WHERE s.user_id = u.id \
                       AND s.last_activity > DATE_SUB(NOW(), INTERVAL 5 MINUTE)) AS online \
         FROM nf_user u \
         LEFT JOIN nf_user_profile up ON up.id = u.id \
         LEFT JOIN nf_file f ON f.id = up.avatar \
         WHERE u.id = ? AND u.deleted = '0'",
    )
    .bind(user_id)
    .fetch_optional(&ctx.state.db)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(Profile {
        id: row.try_get("id")?,
        username: text::clean(&row.try_get::<String, _>("username")?),
        admin: row.try_get::<i64, _>("admin")? != 0,
        registration_date: row.try_get("registration_date")?,
        last_activity_date: row.try_get("last_activity_date")?,
        avatar: row.try_get("avatar")?,
        sex: row.try_get("sex")?,
        quote: text::clean(&row.try_get::<String, _>("quote")?),
        location: text::clean(&row.try_get::<String, _>("location")?),
        website: row.try_get("website")?,
        signature: text::clean(&row.try_get::<String, _>("signature")?),
        online: row.try_get::<i64, _>("online")? != 0,
    }))
}

fn profile_panel(ctx: &Ctx, profile: &Profile) -> Panel {
    let mut details = String::new();
    let mut detail = |icon: &str, label: &str, value: String| {
        details.push_str(&format!(
            "<li class=\"list-group-item\">{} <b>{label}</b> <span class=\"float-right\">{value}</span></li>",
            text::icon(icon)
        ));
    };
    detail(
        "fas fa-shield-alt",
        "Groupe",
        if profile.admin {
            "<span class=\"badge badge-danger\">Administrateur</span>".to_owned()
        } else {
            "<span class=\"badge badge-success\">Membre</span>".to_owned()
        },
    );
    detail("fas fa-sign-in-alt", "Inscription", text::date_short(&profile.registration_date));
    if let Some(date) = &profile.last_activity_date {
        detail("fas fa-history", "Dernière activité", text::date_short(date));
    }
    if !profile.location.is_empty() {
        detail("fas fa-map-marker-alt", "Localisation", profile.location.clone());
    }
    let website = text::decode(&profile.website);
    if website.starts_with("http://") || website.starts_with("https://") {
        detail(
            "fas fa-globe",
            "Site web",
            format!("<a href=\"{0}\" rel=\"nofollow noopener\">{0}</a>", text::encode(&website)),
        );
    }

    let quote = if profile.quote.is_empty() {
        String::new()
    } else {
        format!("<blockquote class=\"mt-3 mb-0\">{}</blockquote>", profile.quote)
    };
    let signature = if profile.signature.is_empty() {
        String::new()
    } else {
        format!(
            "<div class=\"card-body border-top\">{}</div>",
            crate::core::bbcode::render(&profile.signature)
        )
    };
    let body = format!(
        "<div class=\"card-body user-profile\">{}<h4 class=\"mt-3 mb-0\">{}</h4>{quote}</div>\
         <ul class=\"list-group list-group-flush\">{details}</ul>{signature}",
        user::avatar_html(ctx.base(), profile.avatar.as_deref(), profile.sex.as_deref(), profile.online, ""),
        profile.username,
    );
    Panel::new().heading("Profil", ICON).body_raw(body)
}

/// `user` : espace du membre connecté.
pub async fn index(Extension(ctx): Extension<Arc<Ctx>>) -> Result<Response, AppError> {
    let Some(current) = &ctx.user else {
        return Ok(Redirect::to(&ctx.url("user/login")).into_response());
    };
    let Some(profile) = load_profile(&ctx, current.id).await? else {
        return layout::not_found(&ctx).await;
    };
    let page = Page::new("user", TITLE, ICON).blocks(&[Block::Panel(profile_panel(&ctx, &profile))]);
    layout::render(&ctx, page).await
}

/// `user/{id}/{pseudo}` : profil public.
pub async fn profile(
    Extension(ctx): Extension<Arc<Ctx>>,
    Path((id, slug)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let Ok(id) = id.parse::<u32>() else {
        return layout::not_found(&ctx).await;
    };
    let profile = match load_profile(&ctx, id).await? {
        Some(profile) if text::url_title(&profile.username) == slug => profile,
        _ => return layout::not_found(&ctx).await,
    };
    let path = format!("user/{}/{}", profile.id, slug);
    let page = Page::new("user", "Membres", ICON)
        .title(profile.username.clone())
        .crumb(profile.username.clone(), path)
        .blocks(&[Block::Panel(profile_panel(&ctx, &profile))]);
    layout::render(&ctx, page).await
}
