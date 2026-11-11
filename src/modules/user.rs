//! Module Utilisateur : connexion, déconnexion, espace membre et profils publics.
//! Porté de `modules/user`. Inscription et édition du compte : voir `account.rs`.

use crate::{
    core::{
        countries,
        ctx::Ctx,
        layout::{self, Page},
        panel::{Block, Panel},
        password, session, text, throttle, user,
    },
    error::AppError,
};
use axum::{
    extract::Path,
    http::header,
    response::{IntoResponse, Redirect, Response},
    Extension, Form,
};
use chrono::{NaiveDate, NaiveDateTime};
use serde::Deserialize;
use sqlx::Row;
use std::{sync::Arc, time::Duration};

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
         <button type=\"submit\" class=\"btn btn-primary\">Se connecter</button>{register}</form>",
        action = ctx.url("user/login"),
        csrf = ctx.csrf_token(),
        register = if super::account::registration_open(ctx) {
            format!(" <a class=\"btn btn-link\" href=\"{}\">Créer un compte</a>", ctx.url("user/register"))
        } else {
            String::new()
        },
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

pub async fn login_post(
    Extension(ctx): Extension<Arc<Ctx>>,
    Form(form): Form<LoginForm>,
) -> Result<Response, AppError> {
    if !ctx.csrf_ok(&form.csrf) {
        return login_page(&ctx, Some("Votre session a expiré, veuillez réessayer.")).await;
    }
    let state = &ctx.state;
    let throttle_key = format!("login:{}", ctx.ip);
    if throttle::blocked(state, &throttle_key, MAX_FAILED_LOGINS, LOCKOUT) {
        return login_page(&ctx, Some("Trop de tentatives, veuillez patienter quelques minutes.")).await;
    }

    let row = sqlx::query(
        "SELECT id, password, salt FROM nf_user WHERE deleted = '0' AND (username = ? OR email = ?) LIMIT 1",
    )
    // Les pseudos sont stockés encodés en entités (convention NeoFrag), pas les emails.
    .bind(text::encode(form.login.trim()))
    .bind(form.login.trim())
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
        throttle::hit(state, &throttle_key, LOCKOUT);
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

/// Réseaux affichés sur le profil : (colonne, icône, préfixe d'URL).
pub const SOCIALS: &[(&str, &str, &str)] = &[
    ("linkedin", "fab fa-linkedin-in", "https://www.linkedin.com/in/"),
    ("github", "fab fa-github", "https://github.com/"),
    ("instagram", "fab fa-instagram", "https://www.instagram.com/"),
    ("twitch", "fab fa-twitch", "https://www.twitch.tv/"),
];

/// Un identifiant de réseau social sûr à placer dans une URL.
pub fn valid_handle(handle: &str) -> bool {
    !handle.is_empty()
        && handle.len() <= 100
        && handle.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

struct Profile {
    id: u32,
    username: String,
    admin: bool,
    registration_date: NaiveDateTime,
    last_activity_date: Option<NaiveDateTime>,
    avatar: Option<String>,
    sex: Option<String>,
    first_name: String,
    last_name: String,
    date_of_birth: Option<NaiveDate>,
    country: String,
    quote: String,
    location: String,
    website: String,
    signature: String,
    /// Identifiants des réseaux, dans l'ordre de [`SOCIALS`].
    socials: Vec<String>,
    online: bool,
}

async fn load_profile(ctx: &Ctx, user_id: u32) -> Result<Option<Profile>, AppError> {
    let row = sqlx::query(
        "SELECT u.id, u.username, (u.admin = '1') AS admin, CAST(u.registration_date AS DATETIME) AS registration_date, \
                CAST(u.last_activity_date AS DATETIME) AS last_activity_date, \
                f.path AS avatar, up.sex, up.date_of_birth, \
                IFNULL(up.first_name, '') AS first_name, IFNULL(up.last_name, '') AS last_name, \
                IFNULL(up.country, '') AS country, \
                IFNULL(up.quote, '') AS quote, IFNULL(up.location, '') AS location, \
                IFNULL(up.website, '') AS website, IFNULL(up.signature, '') AS signature, \
                IFNULL(up.linkedin, '') AS linkedin, IFNULL(up.github, '') AS github, \
                IFNULL(up.instagram, '') AS instagram, IFNULL(up.twitch, '') AS twitch, \
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
    let mut socials = Vec::with_capacity(SOCIALS.len());
    for (column, _, _) in SOCIALS {
        socials.push(text::decode(&row.try_get::<String, _>(*column)?));
    }
    Ok(Some(Profile {
        id: row.try_get("id")?,
        username: text::clean(&row.try_get::<String, _>("username")?),
        admin: row.try_get::<i64, _>("admin")? != 0,
        registration_date: row.try_get("registration_date")?,
        last_activity_date: row.try_get("last_activity_date")?,
        avatar: row.try_get("avatar")?,
        sex: row.try_get("sex")?,
        first_name: text::clean(&row.try_get::<String, _>("first_name")?),
        last_name: text::clean(&row.try_get::<String, _>("last_name")?),
        date_of_birth: row.try_get("date_of_birth")?,
        country: row.try_get("country")?,
        quote: text::clean(&row.try_get::<String, _>("quote")?),
        location: text::clean(&row.try_get::<String, _>("location")?),
        website: row.try_get("website")?,
        signature: text::clean(&row.try_get::<String, _>("signature")?),
        socials,
        online: row.try_get::<i64, _>("online")? != 0,
    }))
}

/// Âge révolu à la date `today`.
fn age(born: NaiveDate, today: NaiveDate) -> Option<u32> {
    today.years_since(born)
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
    let full_name = format!("{} {}", profile.first_name, profile.last_name);
    if !full_name.trim().is_empty() {
        detail("far fa-user", "Nom", full_name.trim().to_owned());
    }
    if let Some(age) = profile.date_of_birth.and_then(|born| age(born, chrono::Local::now().date_naive())) {
        detail("fas fa-birthday-cake", "Âge", format!("{age} ans"));
    }
    if let Some(country) = countries::name(&profile.country) {
        detail("fas fa-flag", "Pays", text::encode(country));
    }
    if !profile.location.is_empty() {
        detail("fas fa-map-marker-alt", "Localisation", profile.location.clone());
    }
    detail("fas fa-sign-in-alt", "Inscription", text::date_short(&profile.registration_date));
    if let Some(date) = &profile.last_activity_date {
        detail("fas fa-history", "Dernière activité", text::date_short(date));
    }

    let mut socials = String::new();
    let website = text::decode(&profile.website);
    if website.starts_with("http://") || website.starts_with("https://") {
        socials.push_str(&format!(
            "<a class=\"btn website\" href=\"{}\" rel=\"nofollow noopener\" target=\"_blank\">{}</a>",
            text::encode(&website),
            text::icon("fas fa-globe")
        ));
    }
    for ((name, icon, prefix), handle) in SOCIALS.iter().zip(&profile.socials) {
        if valid_handle(handle) {
            socials.push_str(&format!(
                "<a class=\"btn {name}\" href=\"{prefix}{handle}\" rel=\"nofollow noopener\" target=\"_blank\">{}</a>",
                text::icon(icon)
            ));
        }
    }
    if !socials.is_empty() {
        socials = format!("<div class=\"socials\">{socials}</div>");
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
        "<div class=\"card-body user-profile\">{}<h4 class=\"mt-3 mb-0\">{}</h4>{quote}{socials}</div>\
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
    let actions = format!(
        "<a class=\"btn btn-primary\" href=\"{}\">{} Modifier mon profil</a> \
         <a class=\"btn btn-secondary\" href=\"{}\">{} Gérer mon compte</a>",
        ctx.url("user/edit"),
        text::icon("fas fa-edit"),
        ctx.url("user/account"),
        text::icon("fas fa-cogs"),
    );
    let page = Page::new("user", TITLE, ICON).blocks(&[
        Block::Panel(profile_panel(&ctx, &profile)),
        Block::Panel(Panel::new().style("card-transparent text-right").body_raw(actions)),
    ]);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles() {
        assert!(valid_handle("Jnlien"));
        assert!(valid_handle("jean-dupont_01.x"));
        assert!(!valid_handle(""));
        assert!(!valid_handle("a/b"));
        assert!(!valid_handle("x\" onclick=\"y"));
        assert!(!valid_handle("javascript:alert(1)"));
    }

    #[test]
    fn ages() {
        let d = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        assert_eq!(age(d(2000, 11, 12), d(2026, 11, 11)), Some(25));
        assert_eq!(age(d(2000, 11, 11), d(2026, 11, 11)), Some(26));
        assert_eq!(age(d(2030, 1, 1), d(2026, 11, 11)), None);
    }
}
