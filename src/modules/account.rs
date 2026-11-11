//! Compte du membre : inscription, édition du profil, avatar, identifiants, mot de passe.
//! Porté de `modules/user` (formulaires `username`, `email`, `new_password`, `profile`,
//! `profile_socials`, `avatar`, et action `register`).
//!
//! Validation par email et mot de passe oublié : voir `recovery.rs`.
//! Restent à porter : captcha, message de bienvenue, image de couverture (voir ROADMAP).

use super::{
    recovery,
    user::{valid_handle, ICON, SOCIALS, TITLE},
};
use crate::{
    core::{
        bbcode, countries,
        ctx::Ctx,
        files::{self, Fit},
        form,
        layout::{self, Page},
        mail,
        panel::{Block, Panel},
        password, session, text, throttle,
    },
    error::AppError,
};
use axum::{
    extract::{Multipart, Query},
    http::header,
    response::{IntoResponse, Redirect, Response},
    Extension, Form,
};
use chrono::NaiveDate;
use serde::Deserialize;
use sqlx::Row;
use std::{sync::Arc, time::Duration};

type Ctxt = Extension<Arc<Ctx>>;
type Reply = Result<Response, AppError>;

const MIN_PASSWORD: usize = 8;
const MAX_PASSWORD: usize = 200;
const REGISTRATIONS_PER_HOUR: u32 = 3;
const HOUR: Duration = Duration::from_secs(3600);
const SESSION_EXPIRED: &str = "Votre session a expiré, veuillez réessayer.";

/// Les inscriptions sont-elles ouvertes ? Fermées aussi quand le site exige une
/// validation par email sans que l'envoi d'emails soit configuré : créer des comptes
/// non validés contournerait le réglage de l'administrateur.
pub fn registration_open(ctx: &Ctx) -> bool {
    ctx.settings.bool("nf_registration_status") && (!recovery::validation_required(ctx) || mail::enabled(ctx))
}

// ---------------------------------------------------------------------------
// Validation

fn validate_username(username: &str) -> Result<(), &'static str> {
    let length = username.chars().count();
    if !(3..=30).contains(&length) {
        return Err("L'identifiant doit faire entre 3 et 30 caractères.");
    }
    if username.chars().any(char::is_control) || !form::fits(username, 100) {
        return Err("L'identifiant contient des caractères non autorisés.");
    }
    // Le pseudo sert d'adresse au profil (`user/12/pseudo`).
    if text::url_title(username).is_empty() {
        return Err("L'identifiant doit contenir au moins une lettre ou un chiffre.");
    }
    Ok(())
}

pub fn validate_email(email: &str) -> Result<(), &'static str> {
    const INVALID: &str = "Adresse email invalide.";
    let forbidden = |c: char| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"' | '\'' | '&' | ',' | ';');
    if email.len() > 100 || email.chars().any(forbidden) {
        return Err(INVALID);
    }
    match email.split_once('@') {
        Some((local, domain))
            if !local.is_empty()
                && !domain.contains('@')
                && domain.split('.').count() >= 2
                && domain.split('.').all(|label| !label.is_empty()) =>
        {
            Ok(())
        }
        _ => Err(INVALID),
    }
}

pub fn validate_new_password(password: &str, confirm: &str) -> Result<(), &'static str> {
    if password.chars().count() < MIN_PASSWORD {
        return Err("Le mot de passe doit faire au moins 8 caractères.");
    }
    if password.len() > MAX_PASSWORD {
        return Err("Le mot de passe est trop long.");
    }
    if password != confirm {
        return Err("Les mots de passe ne correspondent pas.");
    }
    Ok(())
}

/// Vérifie que l'identifiant et l'email sont libres (hors compte `except`).
async fn check_available(ctx: &Ctx, username: &str, email: &str, except: Option<u32>) -> Result<Result<(), &'static str>, AppError> {
    let except = except.unwrap_or(0);
    let db = &ctx.state.db;
    let taken = sqlx::query("SELECT 1 FROM nf_user WHERE username = ? AND deleted = '0' AND id <> ? LIMIT 1")
        .bind(text::encode(username))
        .bind(except)
        .fetch_optional(db)
        .await?;
    if taken.is_some() {
        return Ok(Err("Identifiant déjà pris"));
    }
    let taken = sqlx::query("SELECT 1 FROM nf_user WHERE email = ? AND deleted = '0' AND id <> ? LIMIT 1")
        .bind(email)
        .bind(except)
        .fetch_optional(db)
        .await?;
    if taken.is_some() {
        return Ok(Err("Adresse email déjà utilisée"));
    }
    Ok(Ok(()))
}

// ---------------------------------------------------------------------------
// Mise en page commune

#[derive(Deserialize, Default)]
pub struct Notice {
    ok: Option<String>,
}

fn form_tag(ctx: &Ctx, action: &str, multipart: bool, fields: &str, submit: &str) -> String {
    format!(
        "<form method=\"post\" action=\"{}\"{}>{}{fields}<button type=\"submit\" class=\"btn btn-primary\">{submit}</button></form>",
        ctx.url(action),
        if multipart { " enctype=\"multipart/form-data\"" } else { "" },
        form::csrf_field(ctx),
    )
}

fn login_redirect(ctx: &Ctx) -> Reply {
    Ok(Redirect::to(&ctx.url("user/login")).into_response())
}

fn menu(ctx: &Ctx, active: &str) -> Panel {
    let entry = |key: &str, path: &str, icon: &str, label: &str| {
        format!(
            "<li class=\"nav-item\"><a class=\"nav-link{}\" href=\"{}\">{} {label}</a></li>",
            if key == active { " active" } else { "" },
            ctx.url(path),
            text::icon(icon)
        )
    };
    Panel::new().style("mb-4").body_raw(format!(
        "<ul class=\"nav\">{}{}{}</ul>",
        entry("index", "user", "fas fa-user", "Mon espace"),
        entry("edit", "user/edit", "fas fa-edit", "Mon profil"),
        entry("account", "user/account", "fas fa-cogs", "Mon compte"),
    ))
}

// ---------------------------------------------------------------------------
// Inscription

#[derive(Deserialize, Default)]
pub struct RegisterForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    password_confirm: String,
    charte: Option<String>,
}

async fn register_page(ctx: &Ctx, values: &RegisterForm, error: Option<&str>) -> Reply {
    let charte = text::clean(ctx.settings.get("nf_registration_charte"));
    let charte_html = if charte.is_empty() {
        String::new()
    } else {
        format!(
            "<div class=\"form-group\"><label><b>Règlement</b></label>\
             <div class=\"border rounded p-3 mb-2\" style=\"max-height: 200px; overflow-y: auto;\">{}</div></div>{}",
            bbcode::render(&charte),
            form::checkbox("J'ai lu et j'accepte le règlement", "charte", values.charte.is_some()),
        )
    };
    let fields = format!(
        "{}{}{}{}{}{charte_html}",
        form::alert("danger", error),
        form::input("Identifiant", "username", &values.username, 30, true, ""),
        form::email("Email", "email", &values.email, true),
        form::password("Mot de passe", "password", "new-password", true, "8 caractères minimum."),
        form::password("Confirmation", "password_confirm", "new-password", true, ""),
    );
    let panel = Panel::new()
        .heading("Créer un compte", "fas fa-sign-in-alt")
        .body(form_tag(ctx, "user/register", false, &fields, "Créer mon compte"));
    let page = Page::new("user", TITLE, ICON)
        .title("Créer un compte")
        .crumb("Créer un compte", "user/register")
        .content(format!(
            "<div class=\"row justify-content-center\"><div class=\"col-12 col-lg-6\">{}</div></div>",
            panel.render()
        ));
    layout::render(ctx, page).await
}

async fn registration_closed(ctx: &Ctx) -> Reply {
    let panel = Panel::new()
        .heading("Créer un compte", "fas fa-sign-in-alt")
        .style("card-info")
        .body("<div class=\"text-center\">Les inscriptions sont actuellement fermées.</div>");
    let page = Page::new("user", TITLE, ICON)
        .title("Créer un compte")
        .crumb("Créer un compte", "user/register")
        .blocks(&[Block::Panel(panel)]);
    layout::render(ctx, page).await
}

pub async fn register_get(Extension(ctx): Ctxt) -> Reply {
    if ctx.user.is_some() {
        return Ok(Redirect::to(&ctx.url("user")).into_response());
    }
    if !registration_open(&ctx) {
        return registration_closed(&ctx).await;
    }
    register_page(&ctx, &RegisterForm::default(), None).await
}

pub async fn register_post(Extension(ctx): Ctxt, Form(mut form): Form<RegisterForm>) -> Reply {
    if ctx.user.is_some() {
        return Ok(Redirect::to(&ctx.url("user")).into_response());
    }
    if !registration_open(&ctx) {
        return registration_closed(&ctx).await;
    }
    form.username = form.username.trim().to_owned();
    form.email = form.email.trim().to_owned();
    if !ctx.csrf_ok(&form.csrf) {
        return register_page(&ctx, &form, Some(SESSION_EXPIRED)).await;
    }

    let throttle_key = format!("register:{}", ctx.ip);
    if throttle::blocked(&ctx.state, &throttle_key, REGISTRATIONS_PER_HOUR, HOUR) {
        return register_page(&ctx, &form, Some("Trop d'inscriptions depuis cette adresse, réessayez plus tard.")).await;
    }

    let charte_required = !ctx.settings.get("nf_registration_charte").trim().is_empty();
    let checked = validate_username(&form.username)
        .and_then(|()| validate_email(&form.email))
        .and_then(|()| validate_new_password(&form.password, &form.password_confirm))
        .and_then(|()| {
            if charte_required && form.charte.is_none() {
                Err("Vous devez accepter le règlement.")
            } else {
                Ok(())
            }
        });
    if let Err(message) = checked {
        return register_page(&ctx, &form, Some(message)).await;
    }
    if let Err(message) = check_available(&ctx, &form.username, &form.email, None).await? {
        return register_page(&ctx, &form, Some(message)).await;
    }

    let (hash, salt) = password::hash(&ctx.state.config.security.password_salt, &form.password);
    let created = sqlx::query("INSERT INTO nf_user (username, password, salt, email, data) VALUES (?, ?, ?, ?, '')")
        .bind(text::encode(&form.username))
        .bind(hash)
        .bind(salt)
        .bind(&form.email)
        .execute(&ctx.state.db)
        .await?;
    throttle::hit(&ctx.state, &throttle_key, HOUR);
    let user_id = created.last_insert_id() as u32;

    if recovery::validation_required(&ctx) {
        if !recovery::send_validation(&ctx, user_id, &form.username, &form.email).await? {
            // Sans cet email le compte serait inutilisable : on l'annule pour permettre un nouvel essai.
            sqlx::query("DELETE FROM nf_user WHERE id = ?")
                .bind(user_id)
                .execute(&ctx.state.db)
                .await?;
            return register_page(&ctx, &form, Some("Une erreur s'est produite lors de l'envoi du message")).await;
        }
        return recovery::notice_page(
            &ctx,
            "Créer un compte",
            "fas fa-sign-in-alt",
            "user/register",
            "Votre compte a été créé. Un email vient de vous être envoyé : cliquez sur le lien qu'il contient pour valider votre inscription.",
        )
        .await;
    }

    let cookie = session::login(&ctx, user_id, false).await?;
    Ok(([(header::SET_COOKIE, cookie.to_string())], Redirect::to(&ctx.url("user/edit?ok=welcome"))).into_response())
}

// ---------------------------------------------------------------------------
// Profil

#[derive(Deserialize, Default)]
pub struct ProfileForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    first_name: String,
    #[serde(default)]
    last_name: String,
    #[serde(default)]
    date_of_birth: String,
    #[serde(default)]
    sex: String,
    #[serde(default)]
    country: String,
    #[serde(default)]
    location: String,
    #[serde(default)]
    quote: String,
    #[serde(default)]
    signature: String,
    #[serde(default)]
    website: String,
    #[serde(default)]
    linkedin: String,
    #[serde(default)]
    github: String,
    #[serde(default)]
    instagram: String,
    #[serde(default)]
    twitch: String,
}

impl ProfileForm {
    fn social(&self, name: &str) -> &str {
        match name {
            "linkedin" => &self.linkedin,
            "github" => &self.github,
            "instagram" => &self.instagram,
            _ => &self.twitch,
        }
    }

    fn trim(&mut self) {
        for field in [
            &mut self.first_name,
            &mut self.last_name,
            &mut self.date_of_birth,
            &mut self.location,
            &mut self.quote,
            &mut self.signature,
            &mut self.website,
            &mut self.linkedin,
            &mut self.github,
            &mut self.instagram,
            &mut self.twitch,
        ] {
            *field = field.trim().to_owned();
        }
    }
}

/// Valeurs actuelles du profil (textes décodés, prêts pour le formulaire) et chemin de l'avatar.
async fn load_profile_form(ctx: &Ctx, user_id: u32) -> Result<(ProfileForm, Option<String>), AppError> {
    let row = sqlx::query(
        "SELECT up.first_name, up.last_name, up.date_of_birth, up.sex, up.country, up.location, up.quote, \
                up.signature, up.website, up.linkedin, up.github, up.instagram, up.twitch, f.path AS avatar \
         FROM nf_user_profile up LEFT JOIN nf_file f ON f.id = up.avatar WHERE up.id = ?",
    )
    .bind(user_id)
    .fetch_optional(&ctx.state.db)
    .await?;
    let Some(row) = row else {
        return Ok((ProfileForm::default(), None));
    };
    let get = |name: &str| row.try_get::<String, _>(name).map(|v| text::decode(&v));
    let form = ProfileForm {
        csrf: String::new(),
        first_name: get("first_name")?,
        last_name: get("last_name")?,
        date_of_birth: row
            .try_get::<Option<NaiveDate>, _>("date_of_birth")?
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default(),
        sex: row.try_get::<Option<String>, _>("sex")?.unwrap_or_default(),
        country: row.try_get("country")?,
        location: get("location")?,
        quote: get("quote")?,
        signature: get("signature")?,
        website: get("website")?,
        linkedin: get("linkedin")?,
        github: get("github")?,
        instagram: get("instagram")?,
        twitch: get("twitch")?,
    };
    Ok((form, row.try_get("avatar")?))
}

async fn edit_page(ctx: &Ctx, values: &ProfileForm, avatar: Option<&str>, error: Option<&str>, notice: Option<&str>) -> Reply {
    if ctx.user.is_none() {
        return login_redirect(ctx);
    }
    let half = |html: String| format!("<div class=\"col-12 col-md-6\">{html}</div>");

    let country_options = std::iter::once(("", "—")).chain(countries::COUNTRIES.iter().copied());
    let socials: String = SOCIALS
        .iter()
        .map(|(name, _, prefix)| {
            let label = match *name {
                "linkedin" => "Linkedin",
                "github" => "GitHub",
                "instagram" => "Instagram",
                _ => "Twitch",
            };
            let help = format!("Nom du compte : {}<b>…</b>", prefix.trim_start_matches("https://"));
            half(form::input(label, name, values.social(name), 100, false, &help))
        })
        .collect();
    let fields = format!(
        "<div class=\"row\">{}{}{}{}{}{}</div>{}{}<legend>Liens</legend>{}<div class=\"row\">{socials}</div>",
        half(form::input("Prénom", "first_name", &values.first_name, 100, false, "")),
        half(form::input("Nom", "last_name", &values.last_name, 100, false, "")),
        half(form::date("Date de naissance", "date_of_birth", &values.date_of_birth)),
        half(form::select("Sexe", "sex", &values.sex, [("", "—"), ("female", "Femme"), ("male", "Homme")])),
        half(form::select("Pays", "country", &values.country, country_options)),
        half(form::input("Localisation", "location", &values.location, 100, false, "")),
        form::input("Citation", "quote", &values.quote, 100, false, ""),
        form::textarea("Signature", "signature", &values.signature, 5, false),
        form::input("Site web", "website", &values.website, 100, false, "Adresse complète, commençant par https://"),
    );
    let profile = Panel::new()
        .heading("Mon profil", "fas fa-edit")
        .body(format!("{}{}", form::alert("danger", error), form_tag(ctx, "user/edit", false, &fields, "Enregistrer")));

    let avatar_url = crate::core::user::avatar_url(ctx.base(), avatar, Some(values.sex.as_str()));
    let remove = if avatar.is_some() {
        format!(
            "<form method=\"post\" action=\"{}\" class=\"mt-2\">{}<button type=\"submit\" class=\"btn btn-sm btn-outline-danger\">{} Supprimer l'avatar</button></form>",
            ctx.url("user/avatar/delete"),
            form::csrf_field(ctx),
            text::icon("far fa-trash-alt")
        )
    } else {
        String::new()
    };
    let avatar_panel = Panel::new().heading("Avatar", "far fa-image").body(format!(
        "{}{remove}",
        form_tag(
            ctx,
            "user/avatar",
            true,
            &form::image("Image", "avatar", Some(&avatar_url), "JPEG, PNG, GIF ou WebP. L'image est recadrée en carré."),
            "Envoyer"
        )
    ));

    let content = format!(
        "{}{}<div class=\"row\"><div class=\"col-12 col-lg-8\">{}</div><div class=\"col-12 col-lg-4\">{}</div></div>",
        menu(ctx, "edit").render(),
        form::alert("success mt-3", notice),
        profile.render(),
        avatar_panel.render(),
    );
    let page = Page::new("user", TITLE, ICON)
        .title("Mon profil")
        .crumb("Mon profil", "user/edit")
        .content(content);
    layout::render(ctx, page).await
}

pub async fn edit_get(Extension(ctx): Ctxt, Query(notice): Query<Notice>) -> Reply {
    let Some(current) = &ctx.user else {
        return login_redirect(&ctx);
    };
    let (values, avatar) = load_profile_form(&ctx, current.id).await?;
    let notice = match notice.ok.as_deref() {
        Some("welcome") => Some("Votre compte a bien été créé, bienvenue ! Vous pouvez compléter votre profil."),
        Some(_) => Some("Modifications enregistrées."),
        None => None,
    };
    edit_page(&ctx, &values, avatar.as_deref(), None, notice).await
}

struct ValidProfile {
    date_of_birth: Option<NaiveDate>,
    sex: Option<&'static str>,
}

fn validate_profile(form: &ProfileForm, today: NaiveDate) -> Result<ValidProfile, &'static str> {
    let short = [&form.first_name, &form.last_name, &form.location, &form.quote];
    if short.iter().any(|value| !form::fits(value, 100) || value.chars().any(char::is_control)) {
        return Err("Un des champs dépasse 100 caractères.");
    }
    if form.signature.chars().count() > 2000 {
        return Err("La signature est limitée à 2000 caractères.");
    }
    let date_of_birth = if form.date_of_birth.is_empty() {
        None
    } else {
        let earliest = NaiveDate::from_ymd_opt(1900, 1, 1).unwrap_or(NaiveDate::MIN);
        match NaiveDate::parse_from_str(&form.date_of_birth, "%Y-%m-%d") {
            Ok(date) if date < today && date >= earliest => Some(date),
            _ => return Err("Date de naissance invalide"),
        }
    };
    let sex = match form.sex.as_str() {
        "" => None,
        "male" => Some("male"),
        "female" => Some("female"),
        _ => return Err("Formulaire invalide."),
    };
    if !form.country.is_empty() && countries::name(&form.country).is_none() {
        return Err("Pays invalide.");
    }
    if !form.website.is_empty() {
        let ok = (form.website.starts_with("https://") || form.website.starts_with("http://"))
            && form::fits(&form.website, 100)
            && !form.website.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '<' | '>'));
        if !ok {
            return Err("Le site web doit être une adresse complète (https://…).");
        }
    }
    for (name, _, _) in SOCIALS {
        let handle = form.social(name);
        if !handle.is_empty() && !valid_handle(handle) {
            return Err("Les liens attendent seulement le nom du compte (lettres, chiffres, tirets, points).");
        }
    }
    Ok(ValidProfile { date_of_birth, sex })
}

pub async fn edit_post(Extension(ctx): Ctxt, Form(mut form): Form<ProfileForm>) -> Reply {
    let Some(current) = &ctx.user else {
        return login_redirect(&ctx);
    };
    form.trim();
    let avatar: Option<String> = load_profile_form(&ctx, current.id).await?.1;
    if !ctx.csrf_ok(&form.csrf) {
        return edit_page(&ctx, &form, avatar.as_deref(), Some(SESSION_EXPIRED), None).await;
    }
    let valid = match validate_profile(&form, chrono::Local::now().date_naive()) {
        Ok(valid) => valid,
        Err(message) => return edit_page(&ctx, &form, avatar.as_deref(), Some(message), None).await,
    };

    sqlx::query(
        "INSERT INTO nf_user_profile (id, first_name, last_name, signature, date_of_birth, sex, country, location, \
                                      quote, website, linkedin, github, instagram, twitch) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON DUPLICATE KEY UPDATE first_name = VALUES(first_name), last_name = VALUES(last_name), \
            signature = VALUES(signature), date_of_birth = VALUES(date_of_birth), sex = VALUES(sex), \
            country = VALUES(country), location = VALUES(location), quote = VALUES(quote), \
            website = VALUES(website), linkedin = VALUES(linkedin), github = VALUES(github), \
            instagram = VALUES(instagram), twitch = VALUES(twitch)",
    )
    .bind(current.id)
    .bind(text::encode(&form.first_name))
    .bind(text::encode(&form.last_name))
    .bind(text::encode(&form.signature))
    .bind(valid.date_of_birth)
    .bind(valid.sex)
    .bind(&form.country)
    .bind(text::encode(&form.location))
    .bind(text::encode(&form.quote))
    .bind(text::encode(&form.website))
    .bind(&form.linkedin)
    .bind(&form.github)
    .bind(&form.instagram)
    .bind(&form.twitch)
    .execute(&ctx.state.db)
    .await?;
    Ok(Redirect::to(&ctx.url("user/edit?ok=1")).into_response())
}

// ---------------------------------------------------------------------------
// Avatar

async fn current_avatar_id(ctx: &Ctx, user_id: u32) -> Result<Option<u32>, AppError> {
    Ok(sqlx::query_scalar::<_, Option<u32>>("SELECT avatar FROM nf_user_profile WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&ctx.state.db)
        .await?
        .flatten())
}

pub async fn avatar_post(Extension(ctx): Ctxt, multipart: Multipart) -> Reply {
    let Some(current) = &ctx.user else {
        return login_redirect(&ctx);
    };
    let (values, avatar) = load_profile_form(&ctx, current.id).await?;
    let fail = |message: &'static str| edit_page(&ctx, &values, avatar.as_deref(), Some(message), None);

    let mut data = match files::read_multipart(multipart).await {
        Ok(data) => data,
        Err(message) => return fail(message).await,
    };
    if !ctx.csrf_ok(data.text("csrf")) {
        return fail(SESSION_EXPIRED).await;
    }
    let Some(upload) = data.take_file("avatar") else {
        return fail("Choisissez une image à envoyer.").await;
    };
    let throttle_key = format!("upload:{}", current.id);
    if throttle::blocked(&ctx.state, &throttle_key, 20, HOUR) {
        return fail("Trop d'envois, réessayez plus tard.").await;
    }
    throttle::hit(&ctx.state, &throttle_key, HOUR);

    let file_id = match files::save_image(&ctx, "user/avatar", upload, Fit::Square(250)).await? {
        Ok(id) => id,
        Err(message) => return fail(message).await,
    };
    let previous = current_avatar_id(&ctx, current.id).await?;
    sqlx::query(
        "INSERT INTO nf_user_profile (id, first_name, last_name, signature, country, location, quote, website, \
                                      linkedin, github, instagram, twitch, avatar) \
         VALUES (?, '', '', '', '', '', '', '', '', '', '', '', ?) \
         ON DUPLICATE KEY UPDATE avatar = VALUES(avatar)",
    )
    .bind(current.id)
    .bind(file_id)
    .execute(&ctx.state.db)
    .await?;
    if let Some(previous) = previous {
        files::delete(&ctx, previous).await?;
    }
    Ok(Redirect::to(&ctx.url("user/edit?ok=1")).into_response())
}

#[derive(Deserialize)]
pub struct CsrfOnly {
    #[serde(default)]
    csrf: String,
}

pub async fn avatar_delete(Extension(ctx): Ctxt, Form(form): Form<CsrfOnly>) -> Reply {
    let Some(current) = &ctx.user else {
        return login_redirect(&ctx);
    };
    if ctx.csrf_ok(&form.csrf) {
        if let Some(file_id) = current_avatar_id(&ctx, current.id).await? {
            // La clé étrangère remet `nf_user_profile.avatar` à NULL.
            files::delete(&ctx, file_id).await?;
        }
    }
    Ok(Redirect::to(&ctx.url("user/edit?ok=1")).into_response())
}

// ---------------------------------------------------------------------------
// Compte : identifiant, email, mot de passe

#[derive(Deserialize, Default)]
pub struct AccountForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    password: String,
}

#[derive(Deserialize, Default)]
pub struct PasswordForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    password_new: String,
    #[serde(default)]
    password_confirm: String,
}

async fn load_account(ctx: &Ctx, user_id: u32) -> Result<AccountForm, AppError> {
    let row = sqlx::query("SELECT username, IFNULL(email, '') AS email FROM nf_user WHERE id = ?")
        .bind(user_id)
        .fetch_one(&ctx.state.db)
        .await?;
    Ok(AccountForm {
        csrf: String::new(),
        username: text::decode(&row.try_get::<String, _>("username")?),
        email: row.try_get("email")?,
        password: String::new(),
    })
}

/// Le mot de passe actuel est-il correct ? Les échecs sont comptés comme des
/// tentatives de connexion (une session volée ne doit pas permettre de le deviner).
async fn confirm_password(ctx: &Ctx, user_id: u32, given: &str) -> Result<Result<(), &'static str>, AppError> {
    let key = format!("login:{}", ctx.ip);
    if throttle::blocked(&ctx.state, &key, 8, Duration::from_secs(15 * 60)) {
        return Ok(Err("Trop de tentatives, veuillez patienter quelques minutes."));
    }
    let row = sqlx::query("SELECT password, salt FROM nf_user WHERE id = ?")
        .bind(user_id)
        .fetch_one(&ctx.state.db)
        .await?;
    let stored: String = row.try_get("password")?;
    let salt: String = row.try_get("salt")?;
    if password::verify(&ctx.state.config.security.password_salt, given, &stored, &salt).is_some() {
        Ok(Ok(()))
    } else {
        throttle::hit(&ctx.state, &key, Duration::from_secs(15 * 60));
        Ok(Err("Mot de passe actuel incorrect."))
    }
}

async fn account_page(ctx: &Ctx, values: &AccountForm, error: Option<&str>, password_error: Option<&str>, notice: Option<&str>) -> Reply {
    let confirm_help = "Requis pour confirmer la modification.";
    let identity = format!(
        "{}{}{}{}",
        form::alert("danger", error),
        form::input("Identifiant", "username", &values.username, 30, true, ""),
        form::email("Email", "email", &values.email, true),
        form::password("Mot de passe actuel", "password", "current-password", true, confirm_help),
    );
    let password = format!(
        "{}{}{}{}",
        form::alert("danger", password_error),
        form::password("Mot de passe actuel", "password", "current-password", true, ""),
        form::password("Nouveau mot de passe", "password_new", "new-password", true, "8 caractères minimum."),
        form::password("Confirmation", "password_confirm", "new-password", true, ""),
    );
    let content = format!(
        "{}{}<div class=\"row\"><div class=\"col-12 col-lg-6\">{}</div><div class=\"col-12 col-lg-6\">{}</div></div>",
        menu(ctx, "account").render(),
        form::alert("success mt-3", notice),
        Panel::new()
            .heading("Identifiants", "fas fa-user")
            .body(form_tag(ctx, "user/account", false, &identity, "Enregistrer"))
            .render(),
        Panel::new()
            .heading("Mot de passe", "fas fa-lock")
            .body(form_tag(ctx, "user/account/password", false, &password, "Changer le mot de passe"))
            .render(),
    );
    let page = Page::new("user", TITLE, ICON)
        .title("Mon compte")
        .crumb("Mon compte", "user/account")
        .content(content);
    layout::render(ctx, page).await
}

pub async fn account_get(Extension(ctx): Ctxt, Query(notice): Query<Notice>) -> Reply {
    let Some(current) = &ctx.user else {
        return login_redirect(&ctx);
    };
    let values = load_account(&ctx, current.id).await?;
    let notice = match notice.ok.as_deref() {
        Some("password") => Some("Mot de passe modifié. Vos autres sessions ont été déconnectées."),
        Some("reset") => Some("Nouveau mot de passe enregistré"),
        Some(_) => Some("Modifications enregistrées."),
        None => None,
    };
    account_page(&ctx, &values, None, None, notice).await
}

pub async fn account_post(Extension(ctx): Ctxt, Form(mut form): Form<AccountForm>) -> Reply {
    let Some(current) = &ctx.user else {
        return login_redirect(&ctx);
    };
    form.username = form.username.trim().to_owned();
    form.email = form.email.trim().to_owned();
    if !ctx.csrf_ok(&form.csrf) {
        return account_page(&ctx, &form, Some(SESSION_EXPIRED), None, None).await;
    }
    if let Err(message) = validate_username(&form.username).and_then(|()| validate_email(&form.email)) {
        return account_page(&ctx, &form, Some(message), None, None).await;
    }
    if let Err(message) = confirm_password(&ctx, current.id, &form.password).await? {
        return account_page(&ctx, &form, Some(message), None, None).await;
    }
    if let Err(message) = check_available(&ctx, &form.username, &form.email, Some(current.id)).await? {
        return account_page(&ctx, &form, Some(message), None, None).await;
    }

    sqlx::query("UPDATE nf_user SET username = ?, email = ? WHERE id = ?")
        .bind(text::encode(&form.username))
        .bind(&form.email)
        .bind(current.id)
        .execute(&ctx.state.db)
        .await?;
    Ok(Redirect::to(&ctx.url("user/account?ok=1")).into_response())
}

pub async fn password_post(Extension(ctx): Ctxt, Form(form): Form<PasswordForm>) -> Reply {
    let Some(current) = &ctx.user else {
        return login_redirect(&ctx);
    };
    let values = load_account(&ctx, current.id).await?;
    if !ctx.csrf_ok(&form.csrf) {
        return account_page(&ctx, &values, None, Some(SESSION_EXPIRED), None).await;
    }
    if let Err(message) = validate_new_password(&form.password_new, &form.password_confirm) {
        return account_page(&ctx, &values, None, Some(message), None).await;
    }
    if let Err(message) = confirm_password(&ctx, current.id, &form.password).await? {
        return account_page(&ctx, &values, None, Some(message), None).await;
    }

    let (hash, salt) = password::hash(&ctx.state.config.security.password_salt, &form.password_new);
    let db = &ctx.state.db;
    sqlx::query("UPDATE nf_user SET password = ?, salt = ? WHERE id = ?")
        .bind(hash)
        .bind(salt)
        .bind(current.id)
        .execute(db)
        .await?;
    // Quiconque utilisait l'ancien mot de passe ailleurs est déconnecté.
    sqlx::query("DELETE FROM nf_session WHERE user_id = ? AND id <> ?")
        .bind(current.id)
        .bind(&ctx.session_id)
        .execute(db)
        .await?;
    Ok(Redirect::to(&ctx.url("user/account?ok=password")).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames() {
        assert!(validate_username("Jnlien").is_ok());
        assert!(validate_username("Été 2026").is_ok());
        assert!(validate_username("ab").is_err());
        assert!(validate_username(&"a".repeat(31)).is_err());
        assert!(validate_username("!!!").is_err());
        assert!(validate_username("a\u{0}bc").is_err());
    }

    #[test]
    fn emails() {
        assert!(validate_email("jean.dupont+test@exemple.fr").is_ok());
        for bad in ["", "a", "a@b", "@b.fr", "a@.fr", "a@b.", "a b@c.fr", "a@b@c.fr", "<a>@b.fr", "a@b.fr,c@d.fr"] {
            assert!(validate_email(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn passwords() {
        assert!(validate_new_password("correct horse", "correct horse").is_ok());
        assert!(validate_new_password("court", "court").is_err());
        assert!(validate_new_password("assez long 1", "assez long 2").is_err());
        assert!(validate_new_password(&"x".repeat(201), &"x".repeat(201)).is_err());
    }

    #[test]
    fn profiles() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 11).unwrap();
        let base = || ProfileForm {
            date_of_birth: "1995-04-02".into(),
            sex: "male".into(),
            country: "fr".into(),
            website: "https://exemple.fr".into(),
            github: "Jnlien".into(),
            ..ProfileForm::default()
        };
        let valid = validate_profile(&base(), today).unwrap();
        assert_eq!(valid.sex, Some("male"));
        assert!(valid.date_of_birth.is_some());
        assert!(validate_profile(&ProfileForm::default(), today).is_ok());

        let bad = [
            ProfileForm { date_of_birth: "2030-01-01".into(), ..base() },
            ProfileForm { date_of_birth: "hier".into(), ..base() },
            ProfileForm { sex: "x".into(), ..base() },
            ProfileForm { country: "zz".into(), ..base() },
            ProfileForm { website: "javascript:alert(1)".into(), ..base() },
            ProfileForm { website: "https://a.fr/\" onclick=\"x".into(), ..base() },
            ProfileForm { github: "../x".into(), ..base() },
            ProfileForm { quote: "x".repeat(101), ..base() },
        ];
        for form in &bad {
            assert!(validate_profile(form, today).is_err());
        }
    }
}
