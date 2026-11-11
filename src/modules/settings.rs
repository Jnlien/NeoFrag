//! Paramètres du site (`modules/settings`), mode maintenance, `robots.txt` et `humans.txt`.
//!
//! Les réglages sont rangés par section ; chaque section est un formulaire.
//! Non repris de l'original : couleurs et fond du thème, clés reCAPTCHA, identifiant
//! Analytics, authentification HTTP.

use super::admin::{self, admin_guard, Ctxt, Notice, Reply};
use crate::{
    core::{
        ctx::Ctx,
        files::{Fit, FormData},
        form::{self, checkbox, input, textarea},
        layout,
        settings::Settings,
        text,
    },
    widgets::extras::SOCIALS,
};
use axum::{
    extract::{Multipart, Path, Query},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    Extension,
};

#[derive(Clone, Copy)]
enum Kind {
    Text(usize),
    Required(usize),
    /// Chemin d'une page du site (`news`, `forum`, chemin d'une page…).
    PagePath,
    Email,
    Url,
    Date,
    Int(u32, u32),
    Bool,
    Area(usize),
}

struct Field {
    name: &'static str,
    label: &'static str,
    kind: Kind,
    help: &'static str,
}

const fn field(name: &'static str, label: &'static str, kind: Kind, help: &'static str) -> Field {
    Field { name, label, kind, help }
}

const GENERAL: &[Field] = &[
    field("nf_name", "Nom du site", Kind::Required(100), ""),
    field("nf_description", "Description", Kind::Text(200), ""),
    field("nf_default_page", "Page d'accueil", Kind::PagePath, "« news », « forum », « members »… ou le chemin d'accès d'une page."),
    field("nf_contact", "Adresse email de contact", Kind::Email, "Destinataire du formulaire de contact."),
    field("nf_robots_txt", "robots.txt", Kind::Area(5000), ""),
    field("nf_humans_txt", "humans.txt", Kind::Area(5000), ""),
];

const LISTS: &[Field] = &[
    field("news_per_page", "Actualités par page", Kind::Int(1, 100), ""),
    field("forum_topics_per_page", "Sujets du forum par page", Kind::Int(1, 100), ""),
    field("forum_messages_per_page", "Messages du forum par page", Kind::Int(1, 100), ""),
    field("images_per_page", "Images de galerie par page", Kind::Int(1, 100), ""),
    field("events_per_page", "Événements par page", Kind::Int(1, 100), ""),
    field("recruits_per_page", "Offres de recrutement par page", Kind::Int(1, 100), ""),
];

const REGISTRATION: &[Field] = &[
    field("nf_registration_status", "Inscriptions ouvertes", Kind::Bool, ""),
    field("nf_registration_validation", "Validation de l'adresse email à l'inscription", Kind::Bool, "Nécessite l'envoi d'emails."),
    field("nf_registration_charte", "Charte à accepter à l'inscription", Kind::Area(20000), "Vide : pas de charte."),
];

const TEAM: &[Field] = &[
    field("nf_team_name", "Nom de l'équipe", Kind::Text(100), ""),
    field("nf_team_type", "Type de structure", Kind::Text(100), "Ex. association, équipe e-sport…"),
    field("nf_team_creation", "Date de création", Kind::Date, ""),
    field("nf_team_biographie", "Présentation", Kind::Area(20000), "BBCode accepté."),
];

const MAINTENANCE: &[Field] = &[
    field("nf_maintenance", "Site en maintenance", Kind::Bool, "Seuls les administrateurs accèdent au site ; la page de connexion reste disponible."),
    field("nf_maintenance_title", "Titre", Kind::Text(100), ""),
    field("nf_maintenance_content", "Message", Kind::Area(5000), "BBCode accepté."),
];

const SECTIONS: [(&str, &str, &str); 6] = [
    ("general", "Général", "fas fa-cog"),
    ("lists", "Listes", "fas fa-list"),
    ("registration", "Inscriptions", "fas fa-user-plus"),
    ("team", "Équipe", "fas fa-headset"),
    ("socials", "Réseaux sociaux", "fas fa-share-alt"),
    ("maintenance", "Maintenance", "fas fa-power-off"),
];

fn social_fields() -> Vec<Field> {
    SOCIALS.iter().map(|(name, title, _, _)| field(name, title, Kind::Url, "")).collect()
}

fn fields_of(section: &str) -> Option<Vec<&'static Field>> {
    let list: &'static [Field] = match section {
        "general" => GENERAL,
        "lists" => LISTS,
        "registration" => REGISTRATION,
        "team" => TEAM,
        "maintenance" => MAINTENANCE,
        // Construite à partir de la liste des réseaux : conservée pour la durée du programme.
        "socials" => {
            static SOCIAL_FIELDS: std::sync::OnceLock<Vec<Field>> = std::sync::OnceLock::new();
            SOCIAL_FIELDS.get_or_init(social_fields)
        }
        _ => return None,
    };
    Some(list.iter().collect())
}

/// Valide une valeur saisie ; renvoie la valeur à enregistrer (texte brut) ou une erreur.
fn validate(kind: Kind, raw: &str) -> Result<String, &'static str> {
    let value = raw.trim();
    let within = |max: usize| value.chars().count() <= max;
    match kind {
        Kind::Text(max) | Kind::Area(max) => within(max).then(|| value.to_owned()).ok_or("texte trop long"),
        Kind::Required(max) => (!value.is_empty() && within(max)).then(|| value.to_owned()).ok_or("valeur obligatoire"),
        Kind::PagePath => {
            let path = value.trim_matches('/');
            let ok = !path.is_empty() && path.len() <= 100 && path.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            ok.then(|| path.to_owned()).ok_or("chemin invalide")
        }
        Kind::Email => (value.is_empty() || super::account::validate_email(value).is_ok())
            .then(|| value.to_owned())
            .ok_or("adresse email invalide"),
        Kind::Url => {
            let ok = value.is_empty()
                || ((value.starts_with("https://") || value.starts_with("http://"))
                    && value.len() <= 255
                    && !value.chars().any(|c| c.is_whitespace() || c.is_control()));
            ok.then(|| value.to_owned()).ok_or("adresse web invalide (elle doit commencer par https://)")
        }
        Kind::Date => (value.is_empty() || chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok())
            .then(|| value.to_owned())
            .ok_or("date invalide"),
        Kind::Int(min, max) => value
            .parse::<u32>()
            .ok()
            .filter(|n| (min..=max).contains(n))
            .map(|n| n.to_string())
            .ok_or("nombre hors limites"),
        Kind::Bool => Ok(value.to_owned()),
    }
}

async fn logo_path(ctx: &Ctx) -> Result<(Option<u32>, Option<String>), sqlx::Error> {
    let id = u32::try_from(ctx.settings.int("nf_team_logo")).ok().filter(|id| *id > 0);
    let Some(id) = id else {
        return Ok((None, None));
    };
    let path: Option<String> = sqlx::query_scalar("SELECT path FROM nf_file WHERE id = ?")
        .bind(id)
        .fetch_optional(&ctx.state.db)
        .await?;
    Ok((path.is_some().then_some(id), path))
}

type Values = Vec<(&'static str, String)>;

async fn page(ctx: &Ctx, section: &str, values: Values, error: Option<String>, notice: Option<&str>) -> Reply {
    let Some(fields) = fields_of(section) else {
        return layout::not_found(ctx).await;
    };
    let tabs: String = SECTIONS
        .iter()
        .map(|(key, title, icon)| {
            format!(
                "<li class=\"nav-item\"><a class=\"nav-link{}\" href=\"{}\">{} {title}</a></li>",
                if *key == section { " active" } else { "" },
                ctx.url(&format!("admin/settings/{key}")),
                text::icon(icon),
            )
        })
        .collect();
    let mut html = String::new();
    for f in &fields {
        let value = values.iter().find(|(name, _)| *name == f.name).map(|(_, v)| v.clone()).unwrap_or_default();
        html.push_str(&match f.kind {
            Kind::Text(max) => input(f.label, f.name, &value, max, false, f.help),
            Kind::Required(max) => input(f.label, f.name, &value, max, true, f.help),
            Kind::PagePath => input(f.label, f.name, &value, 100, true, f.help),
            Kind::Email => form::email(f.label, f.name, &value, false),
            Kind::Url => input(f.label, f.name, &value, 255, false, f.help),
            Kind::Date => form::date(f.label, f.name, &value),
            Kind::Int(_, max) => input(f.label, f.name, &value, max.to_string().len(), true, f.help),
            Kind::Bool => format!(
                "{}{}",
                checkbox(f.label, f.name, value == "1"),
                if f.help.is_empty() { String::new() } else { format!("<p class=\"text-muted\"><small>{}</small></p>", f.help) }
            ),
            Kind::Area(_) => format!(
                "{}{}",
                textarea(f.label, f.name, &value, 6, false),
                if f.help.is_empty() { String::new() } else { format!("<p class=\"text-muted\"><small>{}</small></p>", f.help) }
            ),
        });
    }
    if section == "team" {
        let (_, path) = logo_path(ctx).await?;
        html.push_str(&admin::image_field(ctx, "Logo", "logo", path.as_deref().map(|p| p.trim_start_matches("./")), ""));
    }
    let content = format!(
        "<ul class=\"nav nav-pills mb-4\">{tabs}</ul>{}",
        admin::upload_card(ctx, &format!("admin/settings/{section}"), error.as_deref(), html, "admin"),
    );
    admin::render(ctx, "settings", "Paramètres", notice, content)
}

fn stored(ctx: &Ctx, f: &Field) -> String {
    let raw = ctx.settings.get(f.name);
    match f.kind {
        Kind::Bool => if ctx.settings.bool(f.name) { "1" } else { "0" }.to_owned(),
        _ => text::decode(raw),
    }
}

fn stored_values(ctx: &Ctx, section: &str) -> Values {
    fields_of(section).unwrap_or_default().into_iter().map(|f| (f.name, stored(ctx, f))).collect()
}

pub async fn index(Extension(ctx): Ctxt, Query(notice): Query<Notice>) -> Reply {
    admin_guard!(ctx);
    page(&ctx, "general", stored_values(&ctx, "general"), None, notice.message()).await
}

pub async fn section_get(Extension(ctx): Ctxt, Path(section): Path<String>, Query(notice): Query<Notice>) -> Reply {
    admin_guard!(ctx);
    page(&ctx, &section, stored_values(&ctx, &section), None, notice.message()).await
}

pub async fn section_post(Extension(ctx): Ctxt, Path(section): Path<String>, multipart: Multipart) -> Reply {
    admin_guard!(ctx);
    let Some(fields) = fields_of(&section) else {
        return layout::not_found(&ctx).await;
    };
    let mut data: FormData = admin::read_upload_form(&ctx, multipart).await?;
    let submitted = |f: &Field| match f.kind {
        Kind::Bool => if data.checked(f.name) { "1" } else { "0" }.to_owned(),
        _ => data.text(f.name).to_owned(),
    };

    let mut valid = Vec::with_capacity(fields.len());
    let mut error = None;
    for f in &fields {
        match validate(f.kind, &submitted(f)) {
            Ok(value) => valid.push((*f, value)),
            Err(reason) => {
                error = Some(format!("{} : {reason}.", f.label));
                break;
            }
        }
    }
    if error.is_some() {
        let values: Values = fields.iter().map(|f| (f.name, submitted(f))).collect();
        return page(&ctx, &section, values, error, None).await;
    }

    let db = &ctx.state.db;
    if section == "team" {
        let (old, _) = logo_path(&ctx).await?;
        match admin::image_change(&ctx, &mut data, "logo", "settings", Fit::MaxWidth(800), old).await? {
            Ok(change) => {
                Settings::set(db, "nf_team_logo", &change.value.unwrap_or(0).to_string(), "int").await?;
                change.finish(&ctx).await?;
            }
            Err(message) => {
                let values: Values = valid.iter().map(|(f, v)| (f.name, v.clone())).collect();
                return page(&ctx, &section, values, Some(message.to_owned()), None).await;
            }
        }
    }
    for (f, value) in valid {
        let (value, kind) = match f.kind {
            Kind::Bool => (value, "bool"),
            Kind::Int(..) => (value, "int"),
            _ => (text::encode(&value), "string"),
        };
        Settings::set(db, f.name, &value, kind).await?;
    }
    admin::redirect(&ctx, &format!("admin/settings/{section}?ok=1"))
}

// ---------------------------------------------------------------------------
// Maintenance, robots.txt, humans.txt

/// Le site est-il fermé pour ce visiteur ? (Les administrateurs passent, et la
/// connexion reste possible pour qu'ils puissent s'identifier.)
pub fn closed_for(ctx: &Ctx) -> bool {
    ctx.settings.bool("nf_maintenance") && !ctx.access.admin() && !matches!(ctx.path.as_str(), "user/login" | "user/logout")
}

pub fn maintenance_page(ctx: &Ctx) -> Response {
    let site = text::clean(ctx.settings.get("nf_name"));
    let title = text::clean(ctx.settings.get("nf_maintenance_title"));
    let title = if title.is_empty() { "Site en maintenance".to_owned() } else { title };
    let content = ctx.settings.get("nf_maintenance_content");
    let content = if content.trim().is_empty() {
        "Le site est momentanément indisponible. Merci de revenir un peu plus tard.".to_owned()
    } else {
        crate::core::bbcode::render(&text::clean(content))
    };
    let html = format!(
        "<!DOCTYPE html><html lang=\"fr\"><head><meta charset=\"utf-8\" /><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\
         <meta name=\"robots\" content=\"noindex\" /><title>{title} :: {site}</title>\
         <link rel=\"stylesheet\" href=\"{base}/css/bootstrap.min.css\" /></head>\
         <body class=\"bg-dark text-light\"><div class=\"container text-center\" style=\"padding-top:18vh\">\
         <h1 class=\"mb-4\">{title}</h1><div class=\"lead\">{content}</div>\
         <p class=\"mt-5\"><a class=\"text-muted\" href=\"{login}\">Connexion administrateur</a></p></div></body></html>",
        base = ctx.base(),
        login = ctx.url("user/login"),
    );
    (StatusCode::SERVICE_UNAVAILABLE, [(header::RETRY_AFTER, "3600")], Html(html)).into_response()
}

fn plain(body: String) -> Response {
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response()
}

pub async fn robots_txt(Extension(ctx): Ctxt) -> Response {
    let body = text::decode(ctx.settings.get("nf_robots_txt"));
    plain(if body.trim().is_empty() { "User-agent: *\nDisallow:\n".to_owned() } else { body })
}

pub async fn humans_txt(Extension(ctx): Ctxt) -> Response {
    plain(text::decode(ctx.settings.get("nf_humans_txt")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_validated_by_kind() {
        assert_eq!(validate(Kind::Required(10), "  NeoFrag "), Ok("NeoFrag".to_owned()));
        assert!(validate(Kind::Required(10), " ").is_err());
        assert!(validate(Kind::Text(3), "abcd").is_err());
        assert_eq!(validate(Kind::PagePath, "/news/"), Ok("news".to_owned()));
        assert!(validate(Kind::PagePath, "a/b").is_err());
        assert!(validate(Kind::PagePath, "").is_err());
        assert_eq!(validate(Kind::Int(1, 100), "5"), Ok("5".to_owned()));
        assert!(validate(Kind::Int(1, 100), "0").is_err());
        assert!(validate(Kind::Int(1, 100), "abc").is_err());
        assert!(validate(Kind::Url, "").is_ok());
        assert!(validate(Kind::Url, "https://example.com/x").is_ok());
        assert!(validate(Kind::Url, "javascript:alert(1)").is_err());
        assert!(validate(Kind::Url, "https://a b").is_err());
        assert!(validate(Kind::Date, "2020-02-30").is_err());
        assert!(validate(Kind::Email, "a@b.fr").is_ok());
        assert!(validate(Kind::Email, "nope").is_err());
    }

    #[test]
    fn every_section_has_fields() {
        for (key, _, _) in SECTIONS {
            assert!(fields_of(key).is_some_and(|f| !f.is_empty()), "{key}");
        }
        assert!(fields_of("x").is_none());
    }
}
