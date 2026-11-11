//! Module Partenaires. Porté de `modules/partners` et `widgets/partners`.

use super::admin::{self, admin_guard, Ctxt, CsrfOnly, Notice, Reply};
use crate::{
    core::{
        bbcode,
        ctx::Ctx,
        files::{Fit, FormData},
        form::{fits, input, textarea},
        layout::{self, Page},
        panel::{Block, Panel},
        php::Value,
        text,
    },
    error::AppError,
};
use axum::{
    extract::{Multipart, Path, Query},
    response::{IntoResponse, Redirect},
    Extension, Form,
};
use sqlx::Row;

pub const TITLE: &str = "Partenaires";
pub const ICON: &str = "far fa-handshake";

struct Partner {
    partner_id: u32,
    name: String,
    title: String,
    description: String,
    website: String,
    facebook: String,
    twitter: String,
    code: String,
    count: i64,
    logo_light: Option<String>,
    logo_dark: Option<String>,
    logo_light_id: Option<u32>,
    logo_dark_id: Option<u32>,
}

impl Partner {
    fn path(&self) -> String {
        format!("partners/{}/{}", self.partner_id, self.name)
    }

    /// Logo pour un fond donné, à défaut l'autre.
    fn logo(&self, dark_background: bool) -> Option<&str> {
        let (first, second) = if dark_background {
            (&self.logo_light, &self.logo_dark)
        } else {
            (&self.logo_dark, &self.logo_light)
        };
        first.as_deref().or(second.as_deref())
    }
}

const SELECT: &str = "SELECT p.partner_id, p.name, pl.title, pl.description, p.website, p.facebook, p.twitter, p.code, CAST(p.count AS SIGNED) AS count, \
        p.logo_light AS logo_light_id, p.logo_dark AS logo_dark_id, fl.path AS logo_light, fd.path AS logo_dark \
     FROM nf_partners p INNER JOIN nf_partners_lang pl ON pl.partner_id = p.partner_id \
     LEFT JOIN nf_file fl ON fl.id = p.logo_light LEFT JOIN nf_file fd ON fd.id = p.logo_dark";

fn from_row(row: &sqlx::mysql::MySqlRow) -> sqlx::Result<Partner> {
    let clean = |name: &str| row.try_get::<String, _>(name).map(|v| text::clean(&v));
    Ok(Partner {
        partner_id: row.try_get("partner_id")?,
        name: row.try_get("name")?,
        title: clean("title")?,
        description: clean("description")?,
        website: row.try_get("website")?,
        facebook: row.try_get("facebook")?,
        twitter: row.try_get("twitter")?,
        code: clean("code")?,
        count: row.try_get("count")?,
        logo_light: row.try_get("logo_light")?,
        logo_dark: row.try_get("logo_dark")?,
        logo_light_id: row.try_get("logo_light_id")?,
        logo_dark_id: row.try_get("logo_dark_id")?,
    })
}

async fn all(ctx: &Ctx) -> Result<Vec<Partner>, AppError> {
    let rows = sqlx::query(&format!("{SELECT} ORDER BY p.`order`, pl.title"))
        .fetch_all(&ctx.state.db)
        .await?;
    Ok(rows.iter().map(from_row).collect::<sqlx::Result<_>>()?)
}

async fn find(ctx: &Ctx, id: u32) -> Result<Option<Partner>, AppError> {
    let row = sqlx::query(&format!("{SELECT} WHERE p.partner_id = ?"))
        .bind(id)
        .fetch_optional(&ctx.state.db)
        .await?;
    Ok(row.as_ref().map(from_row).transpose()?)
}

fn is_web_url(url: &str) -> bool {
    let url = text::decode(url);
    (url.starts_with("https://") || url.starts_with("http://"))
        && !url.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '<' | '>'))
}

pub async fn index(Extension(ctx): Ctxt) -> Reply {
    if !ctx.enabled("partners") {
        return layout::not_found(&ctx).await;
    }
    let mut blocks = Vec::new();
    for partner in all(&ctx).await? {
        let logo = match partner.logo(false) {
            Some(path) => format!(
                "<img class=\"mr-4\" src=\"{}\" style=\"max-width: 160px; max-height: 100px;\" alt=\"\" />",
                text::encode(&ctx.url(path))
            ),
            None => String::new(),
        };
        let button = |url: &str, icon: &str, label: &str| {
            if is_web_url(url) {
                format!(
                    "<a class=\"btn btn-light btn-sm\" href=\"{}\" rel=\"nofollow noopener\" target=\"_blank\">{} {label}</a> ",
                    text::encode(&text::decode(url)),
                    text::icon(icon)
                )
            } else {
                String::new()
            }
        };
        let visit = if is_web_url(&partner.website) {
            format!(
                "<a class=\"btn btn-primary btn-sm\" href=\"{}\" rel=\"nofollow noopener\">{} Visiter le site</a> ",
                ctx.url(&partner.path()),
                text::icon("fas fa-globe")
            )
        } else {
            String::new()
        };
        let code = if partner.code.is_empty() {
            String::new()
        } else {
            format!("<p class=\"mt-2 mb-0\">Code promotionnel : <code>{}</code></p>", partner.code)
        };
        blocks.push(Block::Panel(Panel::new().body(format!(
            "<div class=\"media\">{logo}<div class=\"media-body\"><h5>{}</h5>{}<div class=\"mt-2\">{visit}{}{}</div>{code}</div></div>",
            partner.title,
            bbcode::render(&partner.description),
            button(&partner.facebook, "fab fa-facebook-f", "Facebook"),
            button(&partner.twitter, "fab fa-twitter", "Twitter"),
        ))));
    }
    if blocks.is_empty() {
        blocks.push(Block::Panel(
            Panel::new()
                .heading(TITLE, ICON)
                .style("card-info")
                .body("<div class=\"text-center\">Aucun partenaire pour le moment</div>"),
        ));
    }
    layout::render(&ctx, Page::new("partners", TITLE, ICON).blocks(&blocks)).await
}

/// `partners/{id}/{nom}` : compte la visite puis redirige vers le site du partenaire.
pub async fn visit(Extension(ctx): Ctxt, Path((id, name)): Path<(u32, String)>) -> Reply {
    if !ctx.enabled("partners") {
        return layout::not_found(&ctx).await;
    }
    match find(&ctx, id).await? {
        Some(partner) if partner.name == name && is_web_url(&partner.website) => {
            sqlx::query("UPDATE nf_partners SET count = count + 1 WHERE partner_id = ?")
                .bind(id)
                .execute(&ctx.state.db)
                .await?;
            Ok(Redirect::to(&text::decode(&partner.website)).into_response())
        }
        _ => layout::not_found(&ctx).await,
    }
}

/// Widget : logos des partenaires (`display_style` : `light` = logos clairs, pour fond sombre).
pub async fn widget(ctx: &Ctx, settings: &Value, column: bool) -> Result<Vec<Block>, AppError> {
    let dark_background = settings.get_str("display_style") != Some("dark");
    let logos: String = all(ctx)
        .await?
        .iter()
        .filter_map(|p| {
            p.logo(dark_background).map(|path| {
                let href = if is_web_url(&p.website) { ctx.url(&p.path()) } else { ctx.url("partners") };
                format!(
                    "<a class=\"{}\" href=\"{href}\" title=\"{}\"><img src=\"{}\" style=\"max-width: 100%; max-height: 70px;\" alt=\"\" /></a>",
                    if column { "d-block text-center my-3" } else { "d-inline-block mx-3 my-2" },
                    p.title,
                    text::encode(&ctx.url(path))
                )
            })
        })
        .collect();
    if logos.is_empty() {
        return Ok(Vec::new());
    }
    Ok(if column {
        vec![Block::Panel(Panel::new().heading(TITLE, ICON).body(logos))]
    } else {
        vec![Block::Html(format!("<div class=\"text-center\">{logos}</div>"))]
    })
}

// ---------------------------------------------------------------------------
// Administration

pub async fn admin_list(Extension(ctx): Ctxt, Query(notice): Query<Notice>) -> Reply {
    admin_guard!(ctx, "partners");
    let mut body = String::new();
    for p in all(&ctx).await? {
        body.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td>{}</tr>",
            p.title,
            text::clean(&p.website),
            p.count,
            admin::row_actions(&ctx, &format!("admin/{}", p.path()), &format!("admin/partners/delete/{}", p.partner_id), "Supprimer ce partenaire ?"),
        ));
    }
    let content = admin::list_card(&ctx, TITLE, Some("admin/partners/add"), &["Titre", "Site web", "Visites", ""], &body);
    admin::render(&ctx, "partners", TITLE, notice.message(), content)
}

#[derive(Default)]
struct Values {
    title: String,
    description: String,
    website: String,
    facebook: String,
    twitter: String,
    code: String,
}

impl Values {
    fn from_data(data: &FormData) -> Self {
        let get = |name: &str| data.text(name).trim().to_owned();
        Self {
            title: get("title"),
            description: get("description"),
            website: get("website"),
            facebook: get("facebook"),
            twitter: get("twitter"),
            code: get("code"),
        }
    }

    fn validate(&self) -> Result<String, &'static str> {
        if self.title.is_empty() || text::url_title(&self.title).is_empty() || !fits(&self.title, 100) {
            return Err("Le titre est obligatoire (100 caractères maximum).");
        }
        for url in [&self.website, &self.facebook, &self.twitter] {
            if !url.is_empty() && !(is_web_url(url) && fits(url, 100)) {
                return Err("Les adresses doivent être complètes (https://…) et faire 100 caractères au plus.");
            }
        }
        if !fits(&self.code, 50) {
            return Err("Le code est limité à 50 caractères.");
        }
        Ok(text::url_title(&self.title))
    }
}

fn partner_form(ctx: &Ctx, action: &str, v: &Values, partner: Option<&Partner>, error: Option<&str>) -> Reply {
    let fields = format!(
        "{}{}{}{}{}{}{}{}",
        input("Titre", "title", &v.title, 100, true, ""),
        admin::image_field(ctx, "Logo clair", "logo_light", partner.and_then(|p| p.logo_light.as_deref()), "Pour les fonds sombres."),
        admin::image_field(ctx, "Logo foncé", "logo_dark", partner.and_then(|p| p.logo_dark.as_deref()), "Pour les fonds clairs."),
        textarea("Description", "description", &v.description, 6, false),
        input("Site web", "website", &v.website, 100, false, "Adresse complète (https://…)"),
        input("Facebook", "facebook", &v.facebook, 100, false, "Adresse complète"),
        input("Twitter", "twitter", &v.twitter, 100, false, "Adresse complète"),
        input("Code promotionnel", "code", &v.code, 50, false, ""),
    );
    let title = if partner.is_some() { "Modifier le partenaire" } else { "Ajouter un partenaire" };
    admin::render(ctx, "partners", title, None, admin::upload_card(ctx, action, error, fields, "admin/partners"))
}

async fn save(ctx: &Ctx, multipart: Multipart, partner: Option<Partner>) -> Reply {
    let mut data = admin::read_upload_form(ctx, multipart).await?;
    let v = Values::from_data(&data);
    let action = match &partner {
        Some(p) => format!("admin/{}", p.path()),
        None => "admin/partners/add".to_owned(),
    };
    let name = match v.validate() {
        Ok(name) => name,
        Err(message) => return partner_form(ctx, &action, &v, partner.as_ref(), Some(message)),
    };
    let old = partner.as_ref().map(|p| (p.logo_light_id, p.logo_dark_id)).unwrap_or((None, None));
    let light = match admin::image_change(ctx, &mut data, "logo_light", "partners", Fit::MaxWidth(600), old.0).await? {
        Ok(change) => change,
        Err(message) => return partner_form(ctx, &action, &v, partner.as_ref(), Some(message)),
    };
    let dark = match admin::image_change(ctx, &mut data, "logo_dark", "partners", Fit::MaxWidth(600), old.1).await? {
        Ok(change) => change,
        Err(message) => return partner_form(ctx, &action, &v, partner.as_ref(), Some(message)),
    };

    let mut tx = ctx.state.db.begin().await?;
    let partner_id = match &partner {
        Some(p) => {
            sqlx::query(
                "UPDATE nf_partners SET name = ?, logo_light = ?, logo_dark = ?, website = ?, facebook = ?, twitter = ?, code = ? WHERE partner_id = ?",
            )
            .bind(&name)
            .bind(light.value)
            .bind(dark.value)
            .bind(text::encode(&v.website))
            .bind(text::encode(&v.facebook))
            .bind(text::encode(&v.twitter))
            .bind(text::encode(&v.code))
            .bind(p.partner_id)
            .execute(&mut *tx)
            .await?;
            p.partner_id as u64
        }
        None => sqlx::query(
            "INSERT INTO nf_partners (name, logo_light, logo_dark, website, facebook, twitter, code, count, `order`) VALUES (?, ?, ?, ?, ?, ?, ?, 0, 0)",
        )
        .bind(&name)
        .bind(light.value)
        .bind(dark.value)
        .bind(text::encode(&v.website))
        .bind(text::encode(&v.facebook))
        .bind(text::encode(&v.twitter))
        .bind(text::encode(&v.code))
        .execute(&mut *tx)
        .await?
        .last_insert_id(),
    };
    sqlx::query(
        "INSERT INTO nf_partners_lang (partner_id, lang, title, description) VALUES (?, ?, ?, ?) \
         ON DUPLICATE KEY UPDATE title = VALUES(title), description = VALUES(description)",
    )
    .bind(partner_id)
    .bind(ctx.lang())
    .bind(text::encode(&v.title))
    .bind(text::encode(&v.description))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    light.finish(ctx).await?;
    dark.finish(ctx).await?;
    admin::redirect(ctx, "admin/partners?ok=1")
}

pub async fn admin_add_get(Extension(ctx): Ctxt) -> Reply {
    admin_guard!(ctx, "partners");
    partner_form(&ctx, "admin/partners/add", &Values::default(), None, None)
}

pub async fn admin_add_post(Extension(ctx): Ctxt, multipart: Multipart) -> Reply {
    admin_guard!(ctx, "partners");
    save(&ctx, multipart, None).await
}

pub async fn admin_edit_get(Extension(ctx): Ctxt, Path((id, _name)): Path<(u32, String)>) -> Reply {
    admin_guard!(ctx, "partners");
    let Some(p) = find(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let v = Values {
        title: text::decode(&p.title),
        description: text::decode(&p.description),
        website: text::decode(&p.website),
        facebook: text::decode(&p.facebook),
        twitter: text::decode(&p.twitter),
        code: text::decode(&p.code),
    };
    partner_form(&ctx, &format!("admin/{}", p.path()), &v, Some(&p), None)
}

pub async fn admin_edit_post(Extension(ctx): Ctxt, Path((id, _name)): Path<(u32, String)>, multipart: Multipart) -> Reply {
    admin_guard!(ctx, "partners");
    let Some(p) = find(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    save(&ctx, multipart, Some(p)).await
}

pub async fn admin_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx, "partners");
    admin::check_csrf(&ctx, &form.csrf)?;
    let logos = find(&ctx, id).await?.map(|p| [p.logo_light_id, p.logo_dark_id]);
    sqlx::query("DELETE FROM nf_partners WHERE partner_id = ?")
        .bind(id)
        .execute(&ctx.state.db)
        .await?;
    for logo in logos.into_iter().flatten().flatten() {
        crate::core::files::delete(&ctx, logo).await?;
    }
    admin::redirect(&ctx, "admin/partners?ok=1")
}
