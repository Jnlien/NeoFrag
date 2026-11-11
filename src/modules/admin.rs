//! Administration : tableau de bord, actualités, catégories, pages, membres, réglages.
//!
//! Version fonctionnelle mais volontairement sobre : le thème d'administration
//! d'origine (tableaux triables, éditeur BBCode, live editor…) reste à porter, voir
//! ROADMAP.md. Tous les formulaires sont protégés par un jeton anti-CSRF.

use super::{news, pages};
use crate::{
    core::{
        ctx::Ctx,
        files::{self, Fit, FormData},
        form::{checkbox, csrf_field, fits, input, textarea},
        layout,
        text,
    },
    error::AppError,
};
use askama::Template;
use axum::{
    extract::{Multipart, Path, Query},
    response::{Html, IntoResponse, Redirect, Response},
    Extension, Form,
};
use chrono::NaiveDateTime;
use serde::Deserialize;
use sqlx::Row;
use std::sync::Arc;

pub type Ctxt = Extension<Arc<Ctx>>;
pub type Reply = Result<Response, AppError>;

/// Menu de l'administration : (clé, chemin, icône, libellé, module à activer ou "").
const MENU: &[(&str, &str, &str, &str, &str)] = &[
    ("dashboard", "admin", "fas fa-tachometer-alt", "Tableau de bord", ""),
    ("news", "admin/news", "far fa-file-alt", "Actualités", "news"),
    ("pages", "admin/pages", "far fa-file", "Pages", "pages"),
    ("forum", "admin/forum", "fas fa-comments", "Forum", "forum"),
    ("events", "admin/events", "far fa-calendar-alt", "Événements", "events"),
    ("teams", "admin/teams", "fas fa-headset", "Équipes", "teams"),
    ("games", "admin/games", "fas fa-gamepad", "Jeux", "games"),
    ("awards", "admin/awards", "fas fa-trophy", "Palmarès", "awards"),
    ("recruits", "admin/recruits", "fas fa-bullhorn", "Recrutement", "recruits"),
    ("gallery", "admin/gallery", "far fa-image", "Galerie", "gallery"),
    ("partners", "admin/partners", "far fa-handshake", "Partenaires", "partners"),
    ("talks", "admin/talks", "far fa-comment-dots", "Discussions", "talks"),
    ("comments", "admin/comments", "far fa-comments", "Commentaires", ""),
    ("user", "admin/user", "fas fa-users", "Membres", ""),
    ("access", "admin/access", "fas fa-unlock-alt", "Permissions", ""),
    ("live_editor", "admin/live-editor", "fas fa-desktop", "Live Editor", ""),
    ("addons", "admin/addons", "fas fa-puzzle-piece", "Modules", ""),
    ("statistics", "admin/statistics", "fas fa-chart-pie", "Statistiques", ""),
    ("monitoring", "admin/monitoring", "fas fa-heartbeat", "Supervision", ""),
    ("settings", "admin/settings", "fas fa-cogs", "Paramètres", ""),
];

#[derive(Template)]
#[template(path = "admin/layout.html")]
struct AdminTemplate<'a> {
    base: &'a str,
    title: &'a str,
    active: &'a str,
    menu: Vec<(&'a str, &'a str, &'a str, &'a str)>,
    csrf: String,
    username: &'a str,
    notice: Option<&'a str>,
    content: String,
}

#[derive(Deserialize, Default)]
pub struct Notice {
    ok: Option<String>,
}

impl Notice {
    pub fn message(&self) -> Option<&'static str> {
        self.ok.as_ref().map(|_| "Modifications enregistrées.")
    }
}

pub fn render(ctx: &Ctx, active: &str, title: &str, notice: Option<&str>, content: String) -> Reply {
    let html = AdminTemplate {
        base: ctx.base(),
        title,
        active,
        menu: MENU
            .iter()
            .filter(|item| item.4.is_empty() || ctx.enabled(item.4))
            // Un membre délégué ne voit que le tableau de bord et ses modules.
            .filter(|item| ctx.access.admin() || item.0 == "admin" || ctx.access.admin_of(item.0))
            .map(|item| (item.0, item.1, item.2, item.3))
            .collect(),
        csrf: ctx.csrf_token(),
        username: ctx.user.as_ref().map(|u| u.username.as_str()).unwrap_or(""),
        notice,
        content,
    }
    .render()?;
    Ok(Html(html).into_response())
}

/// Refuse l'accès aux non-administrateurs (réponse à renvoyer telle quelle).
pub async fn deny(ctx: &Ctx) -> Option<Reply> {
    if ctx.access.admin() {
        None
    } else {
        Some(layout::forbidden(ctx).await)
    }
}

/// Refuse l'accès à qui ne peut pas administrer ce module (administrateurs, ou groupe
/// auquel l'administration du module a été confiée).
pub async fn deny_module(ctx: &Ctx, module: &str) -> Option<Reply> {
    if ctx.access.admin_of(module) {
        None
    } else {
        Some(layout::forbidden(ctx).await)
    }
}

pub fn check_csrf(ctx: &Ctx, token: &str) -> Result<(), AppError> {
    if ctx.csrf_ok(token) {
        Ok(())
    } else {
        Err(AppError::BadRequest("Jeton de sécurité invalide ou expiré : rechargez la page et réessayez."))
    }
}

pub fn redirect(ctx: &Ctx, path: &str) -> Reply {
    Ok(Redirect::to(&ctx.url(path)).into_response())
}

// ---------------------------------------------------------------------------
// Petits helpers de formulaire (les valeurs sont des textes bruts, encodés ici).

pub fn form_card(ctx: &Ctx, action: &str, error: Option<&str>, fields: String, back: &str) -> String {
    form_card_as(ctx, action, error, fields, back, false)
}

/// Variante `multipart/form-data`, pour les formulaires qui envoient un fichier.
pub fn upload_card(ctx: &Ctx, action: &str, error: Option<&str>, fields: String, back: &str) -> String {
    form_card_as(ctx, action, error, fields, back, true)
}

fn form_card_as(ctx: &Ctx, action: &str, error: Option<&str>, fields: String, back: &str, multipart: bool) -> String {
    format!(
        "<div class=\"card\"><div class=\"card-body\">{}<form method=\"post\" action=\"{}\"{}>{}{fields}\
         <button type=\"submit\" class=\"btn btn-primary\">Enregistrer</button> \
         <a class=\"btn btn-link\" href=\"{}\">Annuler</a></form></div></div>",
        match error {
            Some(message) => format!("<div class=\"alert alert-danger\">{message}</div>"),
            None => String::new(),
        },
        ctx.url(action),
        if multipart { " enctype=\"multipart/form-data\"" } else { "" },
        csrf_field(ctx),
        ctx.url(back),
    )
}

/// Lit un formulaire multipart d'administration et vérifie son jeton.
pub async fn read_upload_form(ctx: &Ctx, multipart: Multipart) -> Result<FormData, AppError> {
    let data = files::read_multipart(multipart).await.map_err(AppError::BadRequest)?;
    check_csrf(ctx, data.text("csrf"))?;
    Ok(data)
}

/// Champs « image » d'un formulaire : envoi, aperçu et case de suppression.
pub fn image_fields(ctx: &Ctx, current: Option<&str>, help: &str) -> String {
    let url = current.map(|path| ctx.url(path));
    let mut html = crate::core::form::image("Image", "image", url.as_deref(), help);
    if current.is_some() {
        html.push_str(&checkbox("Supprimer l'image actuelle", "image_remove", false));
    }
    html
}

/// Enregistre l'image envoyée dans le formulaire, s'il y en a une.
pub async fn save_form_image(ctx: &Ctx, data: &mut FormData, dir: &'static str) -> Result<Result<Option<u32>, &'static str>, AppError> {
    match data.take_file("image") {
        Some(upload) => Ok(files::save_image(ctx, dir, upload, Fit::MaxWidth(1600)).await?.map(Some)),
        None => Ok(Ok(None)),
    }
}

/// Après la mise à jour d'une ligne : supprime l'ancienne image si elle a été
/// remplacée ou si sa suppression a été demandée.
pub async fn drop_old_image(ctx: &Ctx, old: Option<u32>, new: Option<u32>, remove: bool) -> Result<(), AppError> {
    if let Some(old) = old {
        if new.is_some() || remove {
            files::delete(ctx, old).await?;
        }
    }
    Ok(())
}

pub async fn image_of(ctx: &Ctx, sql: &'static str, id: u32) -> Result<(Option<u32>, Option<String>), AppError> {
    let row = sqlx::query(sql).bind(id).fetch_optional(&ctx.state.db).await?;
    Ok(match row {
        Some(row) => (row.try_get("image_id")?, row.try_get("path")?),
        None => (None, None),
    })
}

const NEWS_IMAGE: &str =
    "SELECT n.image_id, f.path FROM nf_news n LEFT JOIN nf_file f ON f.id = n.image_id WHERE n.news_id = ?";
const CATEGORY_IMAGE: &str = "SELECT c.image_id, f.path FROM nf_news_categories c \
     LEFT JOIN nf_file f ON f.id = c.image_id WHERE c.category_id = ?";

/// Champ image nommé, avec aperçu et case « supprimer » (`<champ>_remove`).
pub fn image_field(ctx: &Ctx, label: &str, field: &str, current: Option<&str>, help: &str) -> String {
    let url = current.map(|path| ctx.url(path));
    let mut html = crate::core::form::image(label, field, url.as_deref(), help);
    if current.is_some() {
        html.push_str(&checkbox("Supprimer l'image actuelle", &format!("{field}_remove"), false));
    }
    html
}

/// Résultat d'un champ image : la valeur à écrire dans la colonne, et l'ancien
/// fichier à supprimer **après** la mise à jour de la ligne (certaines clés
/// étrangères vers `nf_file` sont en cascade : supprimer le fichier d'abord
/// emporterait la ligne).
pub struct ImageChange {
    pub value: Option<u32>,
    obsolete: Option<u32>,
}

impl ImageChange {
    pub async fn finish(self, ctx: &Ctx) -> Result<(), AppError> {
        match self.obsolete {
            Some(file_id) => files::delete(ctx, file_id).await,
            None => Ok(()),
        }
    }
}

/// Traite un champ image : nouvel envoi, suppression demandée, ou inchangé.
pub async fn image_change(
    ctx: &Ctx,
    data: &mut FormData,
    field: &str,
    dir: &'static str,
    fit: Fit,
    old: Option<u32>,
) -> Result<Result<ImageChange, &'static str>, AppError> {
    if let Some(upload) = data.take_file(field) {
        return Ok(files::save_image(ctx, dir, upload, fit)
            .await?
            .map(|new| ImageChange { value: Some(new), obsolete: old }));
    }
    if data.checked(&format!("{field}_remove")) {
        return Ok(Ok(ImageChange { value: None, obsolete: old }));
    }
    Ok(Ok(ImageChange { value: old, obsolete: None }))
}

/// Carte « liste » : titre, bouton d'ajout facultatif, tableau.
pub fn list_card(ctx: &Ctx, title: &str, add: Option<&str>, headers: &[&str], rows: &str) -> String {
    let add = match add {
        Some(path) => format!(
            "<a class=\"btn btn-primary float-right\" href=\"{}\">{} Ajouter</a>",
            ctx.url(path),
            text::icon("fas fa-plus")
        ),
        None => String::new(),
    };
    let head: String = headers.iter().map(|h| format!("<th>{h}</th>")).collect();
    let rows = if rows.is_empty() {
        format!("<tr><td colspan=\"{}\" class=\"text-center text-muted\">Aucun élément</td></tr>", headers.len())
    } else {
        rows.to_owned()
    };
    format!(
        "<div class=\"card\"><div class=\"card-body\">{add}<h5>{title}</h5></div>\
         <table class=\"table table-hover mb-0\"><thead><tr>{head}</tr></thead><tbody>{rows}</tbody></table></div>"
    )
}

/// Cellule d'actions d'une ligne : modifier + supprimer.
pub fn row_actions(ctx: &Ctx, edit: &str, delete: &str, confirm: &str) -> String {
    format!(
        "<td class=\"text-right text-nowrap\">{} {}</td>",
        edit_button(ctx, edit),
        delete_button(ctx, delete, confirm)
    )
}

/// À placer en tête de chaque action d'administration : réservé aux administrateurs,
/// et (si un module est nommé) refusé quand ce module est désactivé.
macro_rules! admin_guard {
    ($ctx:expr) => {
        if let Some(denied) = $crate::modules::admin::deny(&$ctx).await {
            return denied;
        }
    };
    ($ctx:expr, $module:literal) => {
        if !$ctx.enabled($module) {
            return $crate::core::layout::not_found(&$ctx).await;
        }
        if let Some(denied) = $crate::modules::admin::deny_module(&$ctx, $module).await {
            return denied;
        }
    };
}
pub(crate) use admin_guard;

pub fn delete_button(ctx: &Ctx, action: &str, confirm: &str) -> String {
    format!(
        "<form method=\"post\" action=\"{}\" class=\"d-inline\" onsubmit=\"return confirm('{}');\">{}\
         <button type=\"submit\" class=\"btn btn-sm btn-outline-danger\" title=\"Supprimer\">{}</button></form>",
        ctx.url(action),
        text::encode(confirm),
        csrf_field(ctx),
        text::icon("far fa-trash-alt"),
    )
}

pub fn edit_button(ctx: &Ctx, path: &str) -> String {
    format!(
        "<a class=\"btn btn-sm btn-outline-secondary\" href=\"{}\" title=\"Modifier\">{}</a>",
        ctx.url(path),
        text::icon("fas fa-edit")
    )
}

pub fn badge(on: bool, yes: &str, no: &str) -> String {
    if on {
        format!("<span class=\"badge badge-success\">{yes}</span>")
    } else {
        format!("<span class=\"badge badge-secondary\">{no}</span>")
    }
}

// ---------------------------------------------------------------------------
// Tableau de bord

pub async fn dashboard(Extension(ctx): Ctxt) -> Reply {
    if !ctx.access.admin_any() {
        return layout::forbidden(&ctx).await;
    }
    let db = &ctx.state.db;
    let count = |sql: &'static str| sqlx::query_scalar::<_, i64>(sql).fetch_one(db);
    let stats = [
        ("Membres", count("SELECT COUNT(*) FROM nf_user WHERE deleted = '0'").await?, "fas fa-users"),
        ("Actualités", count("SELECT COUNT(*) FROM nf_news").await?, "far fa-file-alt"),
        ("Pages", count("SELECT COUNT(*) FROM nf_pages").await?, "far fa-file"),
        (
            "En ligne",
            count("SELECT COUNT(*) FROM nf_session WHERE last_activity > DATE_SUB(NOW(), INTERVAL 5 MINUTE)").await?,
            "fas fa-globe",
        ),
    ];
    let cards: String = stats
        .iter()
        .map(|(label, value, icon)| {
            format!(
                "<div class=\"col-6 col-lg-3 mb-3\"><div class=\"card\"><div class=\"card-body\">\
                 <div class=\"text-muted\">{} {label}</div><div class=\"stat\">{value}</div></div></div></div>",
                text::icon(icon)
            )
        })
        .collect();
    let content = format!(
        "<div class=\"row\">{cards}</div><div class=\"card\"><div class=\"card-body\">\
         <b>NeoFrag (Rust) {}</b> — schéma de base compatible NeoFrag Alpha 0.2.3.</div></div>",
        env!("CARGO_PKG_VERSION")
    );
    render(&ctx, "dashboard", "Tableau de bord", None, content)
}

// ---------------------------------------------------------------------------
// Actualités

pub async fn news_list(Extension(ctx): Ctxt, Query(notice): Query<Notice>) -> Reply {
    if let Some(denied) = deny_module(&ctx, "news").await {
        return denied;
    }
    let db = &ctx.state.db;
    let (items, _) = news::list(db, ctx.lang(), &news::Filter::All, false, 500, 0).await?;
    let categories = news::categories(db, ctx.lang()).await?;

    let mut rows = String::new();
    for item in &items {
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td class=\"text-right text-nowrap\">{} {}</td></tr>",
            item.title,
            item.category_title,
            item.username.as_deref().map(text::clean).unwrap_or_default(),
            text::date_short(&item.date),
            badge(item.published, "Publiée", "Brouillon"),
            edit_button(&ctx, &format!("admin/{}", item.path())),
            delete_button(&ctx, &format!("admin/news/delete/{}", item.news_id), "Supprimer cette actualité ?"),
        ));
    }
    if rows.is_empty() {
        rows.push_str("<tr><td colspan=\"6\" class=\"text-center text-muted\">Aucune actualité</td></tr>");
    }

    let mut category_rows = String::new();
    for category in &categories {
        let path = format!("admin/news/categories/{}/{}", category.category_id, category.name);
        category_rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td class=\"text-right text-nowrap\">{} {}</td></tr>",
            category.title,
            category.nb_news,
            edit_button(&ctx, &path),
            delete_button(
                &ctx,
                &format!("admin/news/categories/delete/{}", category.category_id),
                "Supprimer cette catégorie ET toutes ses actualités ?"
            ),
        ));
    }
    if category_rows.is_empty() {
        category_rows.push_str("<tr><td colspan=\"3\" class=\"text-center text-muted\">Aucune catégorie</td></tr>");
    }

    let content = format!(
        "<div class=\"row\"><div class=\"col-12 col-xl-8\"><div class=\"card\"><div class=\"card-body\">\
         <a class=\"btn btn-primary float-right\" href=\"{}\">{} Ajouter</a><h5>Actualités</h5></div>\
         <table class=\"table table-hover mb-0\"><thead><tr><th>Titre</th><th>Catégorie</th><th>Auteur</th><th>Date</th><th>État</th><th></th></tr></thead>\
         <tbody>{rows}</tbody></table></div></div>\
         <div class=\"col-12 col-xl-4 mt-4 mt-xl-0\"><div class=\"card\"><div class=\"card-body\">\
         <a class=\"btn btn-primary float-right\" href=\"{}\">{} Ajouter</a><h5>Catégories</h5></div>\
         <table class=\"table table-hover mb-0\"><thead><tr><th>Titre</th><th>Actualités</th><th></th></tr></thead>\
         <tbody>{category_rows}</tbody></table></div></div></div>",
        ctx.url("admin/news/add"),
        text::icon("fas fa-plus"),
        ctx.url("admin/news/categories/add"),
        text::icon("fas fa-plus"),
    );
    render(&ctx, "news", "Actualités", notice.message(), content)
}

#[derive(Default)]
pub struct NewsForm {
    title: String,
    category_id: String,
    introduction: String,
    content: String,
    tags: String,
    published: bool,
    /// Image propre à l'actualité (chemin `nf_file.path`), pour l'aperçu.
    image: Option<String>,
}

impl NewsForm {
    fn from_data(data: &FormData, image: Option<String>) -> Self {
        Self {
            title: data.text("title").to_owned(),
            category_id: data.text("category_id").to_owned(),
            introduction: data.text("introduction").to_owned(),
            content: data.text("content").to_owned(),
            tags: data.text("tags").to_owned(),
            published: data.checked("published"),
            image,
        }
    }
}

/// `_tags()` de l'original : séparateurs espace, virgule ou point-virgule, sans doublon.
fn normalize_tags(raw: &str) -> String {
    let mut tags: Vec<&str> = Vec::new();
    for tag in raw.split([' ', ',', ';']).map(str::trim).filter(|t| !t.is_empty()) {
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags.join(",")
}

async fn news_form(ctx: &Ctx, action: &str, form: &NewsForm, error: Option<&str>, title: &str) -> Reply {
    let categories = news::categories(&ctx.state.db, ctx.lang()).await?;
    if categories.is_empty() {
        let content = format!(
            "<div class=\"alert alert-info\">Créez d'abord une <a href=\"{}\">catégorie</a>.</div>",
            ctx.url("admin/news/categories/add")
        );
        return render(ctx, "news", title, None, content);
    }
    let options: String = categories
        .iter()
        .map(|c| {
            format!(
                "<option value=\"{}\"{}>{}</option>",
                c.category_id,
                if c.category_id.to_string() == form.category_id { " selected" } else { "" },
                c.title
            )
        })
        .collect();
    let fields = format!(
        "{}<div class=\"form-group\"><label for=\"f-category\"><b>Catégorie</b></label>\
         <select class=\"form-control\" id=\"f-category\" name=\"category_id\">{options}</select></div>{}{}{}{}{}",
        input("Titre", "title", &form.title, 100, true, ""),
        image_fields(ctx, form.image.as_deref(), "Facultative : sans image, celle de la catégorie est utilisée."),
        textarea("Introduction", "introduction", &form.introduction, 5, true),
        textarea("Contenu", "content", &form.content, 12, false),
        input("Tags", "tags", &form.tags, 250, false, "Séparés par des virgules"),
        checkbox("Publiée", "published", form.published),
    );
    render(ctx, "news", title, None, upload_card(ctx, action, error, fields, "admin/news"))
}

fn validate_news(form: &NewsForm) -> Result<u32, &'static str> {
    if form.title.trim().is_empty() || form.introduction.trim().is_empty() {
        return Err("Le titre et l'introduction sont obligatoires.");
    }
    if !fits(form.title.trim(), 100) {
        return Err("Le titre est trop long (100 caractères maximum).");
    }
    form.category_id.parse().map_err(|_| "Catégorie invalide.")
}

async fn category_exists(ctx: &Ctx, category_id: u32) -> Result<bool, AppError> {
    Ok(sqlx::query("SELECT 1 FROM nf_news_categories WHERE category_id = ?")
        .bind(category_id)
        .fetch_optional(&ctx.state.db)
        .await?
        .is_some())
}

pub async fn news_add_get(Extension(ctx): Ctxt) -> Reply {
    if let Some(denied) = deny_module(&ctx, "news").await {
        return denied;
    }
    let form = NewsForm {
        published: true,
        ..NewsForm::default()
    };
    news_form(&ctx, "admin/news/add", &form, None, "Ajouter une actualité").await
}

pub async fn news_add_post(Extension(ctx): Ctxt, multipart: Multipart) -> Reply {
    if let Some(denied) = deny_module(&ctx, "news").await {
        return denied;
    }
    let mut data = read_upload_form(&ctx, multipart).await?;
    let form = NewsForm::from_data(&data, None);
    let title = "Ajouter une actualité";
    let category_id = match validate_news(&form) {
        Ok(id) if category_exists(&ctx, id).await? => id,
        Ok(_) => return news_form(&ctx, "admin/news/add", &form, Some("Catégorie invalide."), title).await,
        Err(message) => return news_form(&ctx, "admin/news/add", &form, Some(message), title).await,
    };
    let Some(author) = &ctx.user else {
        return layout::forbidden(&ctx).await;
    };
    let image_id = match save_form_image(&ctx, &mut data, "news").await? {
        Ok(image_id) => image_id,
        Err(message) => return news_form(&ctx, "admin/news/add", &form, Some(message), title).await,
    };

    let mut tx = ctx.state.db.begin().await?;
    let result = sqlx::query("INSERT INTO nf_news (category_id, user_id, image_id, published) VALUES (?, ?, ?, ?)")
        .bind(category_id)
        .bind(author.id)
        .bind(image_id)
        .bind(if form.published { "1" } else { "0" })
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO nf_news_lang (news_id, lang, title, introduction, content, tags) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(result.last_insert_id())
        .bind(ctx.lang())
        .bind(text::encode(form.title.trim()))
        .bind(text::encode(form.introduction.trim()))
        .bind(text::encode(form.content.trim()))
        .bind(text::encode(&normalize_tags(&form.tags)))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    redirect(&ctx, "admin/news?ok=1")
}

pub async fn news_edit_get(Extension(ctx): Ctxt, Path((id, _slug)): Path<(String, String)>) -> Reply {
    if let Some(denied) = deny_module(&ctx, "news").await {
        return denied;
    }
    let Ok(id) = id.parse::<u32>() else {
        return layout::not_found(&ctx).await;
    };
    let Some(item) = news::find(&ctx.state.db, ctx.lang(), id, false).await? else {
        return layout::not_found(&ctx).await;
    };
    let form = NewsForm {
        title: text::decode(&item.title),
        category_id: item.category_id.to_string(),
        introduction: text::decode(&item.introduction),
        content: text::decode(&item.content),
        tags: text::decode(&item.tags),
        published: item.published,
        image: image_of(&ctx, NEWS_IMAGE, id).await?.1,
    };
    news_form(&ctx, &format!("admin/{}", item.path()), &form, None, "Modifier l'actualité").await
}

pub async fn news_edit_post(
    Extension(ctx): Ctxt,
    Path((id, slug)): Path<(String, String)>,
    multipart: Multipart,
) -> Reply {
    if let Some(denied) = deny_module(&ctx, "news").await {
        return denied;
    }
    let mut data = read_upload_form(&ctx, multipart).await?;
    let Ok(id) = id.parse::<u32>() else {
        return layout::not_found(&ctx).await;
    };
    let db = &ctx.state.db;
    if news::find(db, ctx.lang(), id, false).await?.is_none() {
        return layout::not_found(&ctx).await;
    }
    let (old_image, old_path) = image_of(&ctx, NEWS_IMAGE, id).await?;
    let form = NewsForm::from_data(&data, old_path);
    let action = format!("admin/news/{id}/{}", text::url_title(&slug));
    let title = "Modifier l'actualité";
    let category_id = match validate_news(&form) {
        Ok(cid) if category_exists(&ctx, cid).await? => cid,
        Ok(_) => return news_form(&ctx, &action, &form, Some("Catégorie invalide."), title).await,
        Err(message) => return news_form(&ctx, &action, &form, Some(message), title).await,
    };

    let new_image = match save_form_image(&ctx, &mut data, "news").await? {
        Ok(image_id) => image_id,
        Err(message) => return news_form(&ctx, &action, &form, Some(message), title).await,
    };

    let mut tx = db.begin().await?;
    sqlx::query("UPDATE nf_news SET category_id = ?, published = ?, image_id = IFNULL(?, image_id) WHERE news_id = ?")
        .bind(category_id)
        .bind(if form.published { "1" } else { "0" })
        .bind(new_image)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE nf_news_lang SET title = ?, introduction = ?, content = ?, tags = ? WHERE news_id = ? AND lang = ?")
        .bind(text::encode(form.title.trim()))
        .bind(text::encode(form.introduction.trim()))
        .bind(text::encode(form.content.trim()))
        .bind(text::encode(&normalize_tags(&form.tags)))
        .bind(id)
        .bind(ctx.lang())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    drop_old_image(&ctx, old_image, new_image, data.checked("image_remove")).await?;
    redirect(&ctx, "admin/news?ok=1")
}

#[derive(Deserialize)]
pub struct CsrfOnly {
    #[serde(default)]
    pub csrf: String,
}

pub async fn news_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    if let Some(denied) = deny_module(&ctx, "news").await {
        return denied;
    }
    check_csrf(&ctx, &form.csrf)?;
    let (image, _) = image_of(&ctx, NEWS_IMAGE, id).await?;
    let mut tx = ctx.state.db.begin().await?;
    sqlx::query("DELETE FROM nf_comment WHERE module = 'news' AND module_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM nf_news WHERE news_id = ?").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    drop_old_image(&ctx, image, None, true).await?;
    redirect(&ctx, "admin/news?ok=1")
}

// ---------------------------------------------------------------------------
// Catégories d'actualités

#[derive(Default)]
pub struct CategoryForm {
    title: String,
    image: Option<String>,
}

fn category_form(ctx: &Ctx, action: &str, form: &CategoryForm, error: Option<&str>, title: &str) -> Reply {
    let fields = format!(
        "{}{}",
        input("Titre", "title", &form.title, 100, true, ""),
        image_fields(ctx, form.image.as_deref(), "Illustre les actualités de la catégorie qui n'ont pas leur propre image."),
    );
    render(ctx, "news", title, None, upload_card(ctx, action, error, fields, "admin/news"))
}

fn validate_category(form: &CategoryForm) -> Result<String, &'static str> {
    let title = form.title.trim();
    if title.is_empty() || text::url_title(title).is_empty() {
        return Err("Le titre est obligatoire.");
    }
    if !fits(title, 100) {
        return Err("Le titre est trop long (100 caractères maximum).");
    }
    Ok(text::url_title(title))
}

pub async fn category_add_get(Extension(ctx): Ctxt) -> Reply {
    if let Some(denied) = deny_module(&ctx, "news").await {
        return denied;
    }
    category_form(&ctx, "admin/news/categories/add", &CategoryForm::default(), None, "Ajouter une catégorie")
}

pub async fn category_add_post(Extension(ctx): Ctxt, multipart: Multipart) -> Reply {
    if let Some(denied) = deny_module(&ctx, "news").await {
        return denied;
    }
    let mut data = read_upload_form(&ctx, multipart).await?;
    let form = CategoryForm {
        title: data.text("title").to_owned(),
        image: None,
    };
    let fail = |message| category_form(&ctx, "admin/news/categories/add", &form, Some(message), "Ajouter une catégorie");
    let name = match validate_category(&form) {
        Ok(name) => name,
        Err(message) => return fail(message),
    };
    let image_id = match save_form_image(&ctx, &mut data, "news/categories").await? {
        Ok(image_id) => image_id,
        Err(message) => return fail(message),
    };
    let mut tx = ctx.state.db.begin().await?;
    let result = sqlx::query("INSERT INTO nf_news_categories (name, image_id) VALUES (?, ?)")
        .bind(&name)
        .bind(image_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO nf_news_categories_lang (category_id, lang, title) VALUES (?, ?, ?)")
        .bind(result.last_insert_id())
        .bind(ctx.lang())
        .bind(text::encode(form.title.trim()))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    redirect(&ctx, "admin/news?ok=1")
}

pub async fn category_edit_get(Extension(ctx): Ctxt, Path((id, _name)): Path<(String, String)>) -> Reply {
    if let Some(denied) = deny_module(&ctx, "news").await {
        return denied;
    }
    let category = news::categories(&ctx.state.db, ctx.lang())
        .await?
        .into_iter()
        .find(|c| c.category_id.to_string() == id);
    let Some(category) = category else {
        return layout::not_found(&ctx).await;
    };
    let form = CategoryForm {
        title: text::decode(&category.title),
        image: image_of(&ctx, CATEGORY_IMAGE, category.category_id).await?.1,
    };
    let action = format!("admin/news/categories/{}/{}", category.category_id, category.name);
    category_form(&ctx, &action, &form, None, "Modifier la catégorie")
}

pub async fn category_edit_post(
    Extension(ctx): Ctxt,
    Path((id, _name)): Path<(String, String)>,
    multipart: Multipart,
) -> Reply {
    if let Some(denied) = deny_module(&ctx, "news").await {
        return denied;
    }
    let mut data = read_upload_form(&ctx, multipart).await?;
    let Ok(id) = id.parse::<u32>() else {
        return layout::not_found(&ctx).await;
    };
    if !category_exists(&ctx, id).await? {
        return layout::not_found(&ctx).await;
    }
    let (old_image, old_path) = image_of(&ctx, CATEGORY_IMAGE, id).await?;
    let form = CategoryForm {
        title: data.text("title").to_owned(),
        image: old_path,
    };
    let action = format!("admin/news/categories/{id}/x");
    let fail = |message| category_form(&ctx, &action, &form, Some(message), "Modifier la catégorie");
    let name = match validate_category(&form) {
        Ok(name) => name,
        Err(message) => return fail(message),
    };
    let new_image = match save_form_image(&ctx, &mut data, "news/categories").await? {
        Ok(image_id) => image_id,
        Err(message) => return fail(message),
    };
    let mut tx = ctx.state.db.begin().await?;
    sqlx::query("UPDATE nf_news_categories SET name = ?, image_id = IFNULL(?, image_id) WHERE category_id = ?")
        .bind(&name)
        .bind(new_image)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO nf_news_categories_lang (category_id, lang, title) VALUES (?, ?, ?) \
         ON DUPLICATE KEY UPDATE title = VALUES(title)",
    )
    .bind(id)
    .bind(ctx.lang())
    .bind(text::encode(form.title.trim()))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    drop_old_image(&ctx, old_image, new_image, data.checked("image_remove")).await?;
    redirect(&ctx, "admin/news?ok=1")
}

pub async fn category_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    if let Some(denied) = deny_module(&ctx, "news").await {
        return denied;
    }
    check_csrf(&ctx, &form.csrf)?;
    // Images à supprimer du disque : celle de la catégorie et celles de ses actualités.
    let mut images: Vec<u32> = sqlx::query_scalar("SELECT image_id FROM nf_news WHERE category_id = ? AND image_id IS NOT NULL")
        .bind(id)
        .fetch_all(&ctx.state.db)
        .await?;
    images.extend(image_of(&ctx, CATEGORY_IMAGE, id).await?.0);
    let mut tx = ctx.state.db.begin().await?;
    sqlx::query(
        "DELETE FROM nf_comment WHERE module = 'news' AND module_id IN (SELECT news_id FROM nf_news WHERE category_id = ?)",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    // Les actualités de la catégorie partent avec elle (clé étrangère en cascade).
    sqlx::query("DELETE FROM nf_news_categories WHERE category_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    for image in images {
        files::delete(&ctx, image).await?;
    }
    redirect(&ctx, "admin/news?ok=1")
}

// ---------------------------------------------------------------------------
// Pages

pub async fn pages_list(Extension(ctx): Ctxt, Query(notice): Query<Notice>) -> Reply {
    if let Some(denied) = deny_module(&ctx, "pages").await {
        return denied;
    }
    let rows = sqlx::query(
        "SELECT p.page_id, p.name, (p.published = '1') AS is_published, pl.title, pl.subtitle FROM nf_pages p \
         INNER JOIN nf_pages_lang pl ON p.page_id = pl.page_id AND pl.lang = ? ORDER BY pl.title",
    )
    .bind(ctx.lang())
    .fetch_all(&ctx.state.db)
    .await?;

    let mut body = String::new();
    for row in &rows {
        let id: u32 = row.try_get("page_id")?;
        let name: String = row.try_get("name")?;
        let title = text::clean(&row.try_get::<String, _>("title")?);
        body.push_str(&format!(
            "<tr><td>{title}</td><td><a href=\"{}\"><code>/{}</code></a></td><td>{}</td><td class=\"text-right text-nowrap\">{} {}</td></tr>",
            text::encode(&ctx.url(&name)),
            text::clean(&name),
            badge(row.try_get::<i64, _>("is_published")? != 0, "Publiée", "Brouillon"),
            edit_button(&ctx, &format!("admin/pages/{id}/{}", text::url_title(&title))),
            delete_button(&ctx, &format!("admin/pages/delete/{id}"), "Supprimer cette page ?"),
        ));
    }
    if body.is_empty() {
        body.push_str("<tr><td colspan=\"4\" class=\"text-center text-muted\">Aucune page</td></tr>");
    }
    let content = format!(
        "<div class=\"card\"><div class=\"card-body\"><a class=\"btn btn-primary float-right\" href=\"{}\">{} Ajouter</a><h5>Pages</h5></div>\
         <table class=\"table table-hover mb-0\"><thead><tr><th>Titre</th><th>Adresse</th><th>État</th><th></th></tr></thead>\
         <tbody>{body}</tbody></table></div>",
        ctx.url("admin/pages/add"),
        text::icon("fas fa-plus"),
    );
    render(&ctx, "pages", "Pages", notice.message(), content)
}

#[derive(Deserialize, Default)]
pub struct PageForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    subtitle: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    content: String,
    published: Option<String>,
}

fn page_form(ctx: &Ctx, action: &str, form: &PageForm, error: Option<&str>, title: &str) -> Reply {
    let fields = format!(
        "{}{}{}{}{}",
        input("Titre", "title", &form.title, 100, true, ""),
        input("Sous-titre", "subtitle", &form.subtitle, 100, false, ""),
        input(
            "Chemin d'accès",
            "name",
            &form.name,
            100,
            false,
            "Adresse de la page (ex. « reglement »). Laissez vide pour la déduire du titre."
        ),
        textarea("Contenu", "content", &form.content, 16, true),
        checkbox("Publiée", "published", form.published.is_some()),
    );
    render(ctx, "pages", title, None, form_card(ctx, action, error, fields, "admin/pages"))
}

/// Valide le formulaire et renvoie le nom (slug) de la page.
async fn validate_page(ctx: &Ctx, form: &PageForm, current_id: Option<u32>) -> Result<Result<String, &'static str>, AppError> {
    if form.title.trim().is_empty() || form.content.trim().is_empty() {
        return Ok(Err("Le titre et le contenu sont obligatoires."));
    }
    if !fits(form.title.trim(), 100) || !fits(form.subtitle.trim(), 100) {
        return Ok(Err("Le titre et le sous-titre sont limités à 100 caractères."));
    }
    let name = match text::url_title(&form.name) {
        name if name.is_empty() => text::url_title(&form.title),
        name => name,
    };
    if name.is_empty() || name.len() > 100 {
        return Ok(Err("Chemin d'accès invalide."));
    }
    if pages::RESERVED.contains(&name.as_str()) {
        return Ok(Err("Ce chemin d'accès est réservé par le site, choisissez-en un autre."));
    }
    let taken = sqlx::query("SELECT page_id FROM nf_pages WHERE name = ?")
        .bind(&name)
        .fetch_optional(&ctx.state.db)
        .await?
        .map(|row| row.try_get::<u32, _>("page_id"))
        .transpose()?;
    if taken.is_some() && taken != current_id {
        return Ok(Err("Une autre page utilise déjà ce chemin d'accès."));
    }
    Ok(Ok(name))
}

pub async fn page_add_get(Extension(ctx): Ctxt) -> Reply {
    if let Some(denied) = deny_module(&ctx, "pages").await {
        return denied;
    }
    let form = PageForm {
        published: Some("on".to_owned()),
        ..PageForm::default()
    };
    page_form(&ctx, "admin/pages/add", &form, None, "Ajouter une page")
}

pub async fn page_add_post(Extension(ctx): Ctxt, Form(form): Form<PageForm>) -> Reply {
    if let Some(denied) = deny_module(&ctx, "pages").await {
        return denied;
    }
    check_csrf(&ctx, &form.csrf)?;
    let name = match validate_page(&ctx, &form, None).await? {
        Ok(name) => name,
        Err(message) => return page_form(&ctx, "admin/pages/add", &form, Some(message), "Ajouter une page"),
    };

    let mut tx = ctx.state.db.begin().await?;
    let result = sqlx::query("INSERT INTO nf_pages (name, published) VALUES (?, ?)")
        .bind(&name)
        .bind(if form.published.is_some() { "1" } else { "0" })
        .execute(&mut *tx)
        .await?;
    let page_id = result.last_insert_id();
    sqlx::query("INSERT INTO nf_pages_lang (page_id, lang, title, subtitle, content) VALUES (?, ?, ?, ?, ?)")
        .bind(page_id)
        .bind(ctx.lang())
        .bind(text::encode(form.title.trim()))
        .bind(text::encode(form.subtitle.trim()))
        .bind(text::encode(form.content.trim()))
        .execute(&mut *tx)
        .await?;
    // Permission « accès au contenu » ouverte à tous par défaut, comme `access->init('pages', 'page')`.
    sqlx::query("INSERT IGNORE INTO nf_access (id, module, action) VALUES (?, 'pages', 'access_page')")
        .bind(page_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    redirect(&ctx, "admin/pages?ok=1")
}

pub async fn page_edit_get(Extension(ctx): Ctxt, Path((id, _slug)): Path<(String, String)>) -> Reply {
    if let Some(denied) = deny_module(&ctx, "pages").await {
        return denied;
    }
    let row = sqlx::query(
        "SELECT p.page_id, p.name, (p.published = '1') AS is_published, pl.title, pl.subtitle, pl.content FROM nf_pages p \
         INNER JOIN nf_pages_lang pl ON p.page_id = pl.page_id AND pl.lang = ? WHERE p.page_id = ?",
    )
    .bind(ctx.lang())
    .bind(id.parse::<u32>().unwrap_or(0))
    .fetch_optional(&ctx.state.db)
    .await?;
    let Some(row) = row else {
        return layout::not_found(&ctx).await;
    };
    let page_id: u32 = row.try_get("page_id")?;
    let form = PageForm {
        csrf: String::new(),
        title: text::decode(&row.try_get::<String, _>("title")?),
        subtitle: text::decode(&row.try_get::<String, _>("subtitle")?),
        name: row.try_get("name")?,
        content: text::decode(&row.try_get::<String, _>("content")?),
        published: (row.try_get::<i64, _>("is_published")? != 0).then(|| "on".to_owned()),
    };
    page_form(&ctx, &format!("admin/pages/{page_id}/edit"), &form, None, "Modifier la page")
}

pub async fn page_edit_post(
    Extension(ctx): Ctxt,
    Path((id, _slug)): Path<(String, String)>,
    Form(form): Form<PageForm>,
) -> Reply {
    if let Some(denied) = deny_module(&ctx, "pages").await {
        return denied;
    }
    check_csrf(&ctx, &form.csrf)?;
    let Ok(id) = id.parse::<u32>() else {
        return layout::not_found(&ctx).await;
    };
    let db = &ctx.state.db;
    if sqlx::query("SELECT 1 FROM nf_pages WHERE page_id = ?").bind(id).fetch_optional(db).await?.is_none() {
        return layout::not_found(&ctx).await;
    }
    let name = match validate_page(&ctx, &form, Some(id)).await? {
        Ok(name) => name,
        Err(message) => {
            return page_form(&ctx, &format!("admin/pages/{id}/edit"), &form, Some(message), "Modifier la page")
        }
    };

    let mut tx = db.begin().await?;
    sqlx::query("UPDATE nf_pages SET name = ?, published = ? WHERE page_id = ?")
        .bind(&name)
        .bind(if form.published.is_some() { "1" } else { "0" })
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO nf_pages_lang (page_id, lang, title, subtitle, content) VALUES (?, ?, ?, ?, ?) \
         ON DUPLICATE KEY UPDATE title = VALUES(title), subtitle = VALUES(subtitle), content = VALUES(content)",
    )
    .bind(id)
    .bind(ctx.lang())
    .bind(text::encode(form.title.trim()))
    .bind(text::encode(form.subtitle.trim()))
    .bind(text::encode(form.content.trim()))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    redirect(&ctx, "admin/pages?ok=1")
}

pub async fn page_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    if let Some(denied) = deny_module(&ctx, "pages").await {
        return denied;
    }
    check_csrf(&ctx, &form.csrf)?;
    let mut tx = ctx.state.db.begin().await?;
    sqlx::query("DELETE FROM nf_access WHERE module = 'pages' AND id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM nf_pages WHERE page_id = ?").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    redirect(&ctx, "admin/pages?ok=1")
}

// ---------------------------------------------------------------------------
// Membres

pub async fn users(Extension(ctx): Ctxt, Query(notice): Query<Notice>) -> Reply {
    if let Some(denied) = deny(&ctx).await {
        return denied;
    }
    let rows = sqlx::query(
        "SELECT id, username, IFNULL(email, '') AS email, (admin = '1') AS admin, \
                CAST(registration_date AS DATETIME) AS registration_date, \
                CAST(last_activity_date AS DATETIME) AS last_activity_date \
         FROM nf_user WHERE deleted = '0' ORDER BY username LIMIT 1000",
    )
    .fetch_all(&ctx.state.db)
    .await?;
    let me = ctx.user.as_ref().map(|u| u.id);

    let mut body = String::new();
    for row in &rows {
        let id: u32 = row.try_get("id")?;
        let username = text::clean(&row.try_get::<String, _>("username")?);
        let registered: NaiveDateTime = row.try_get("registration_date")?;
        let last: Option<NaiveDateTime> = row.try_get("last_activity_date")?;
        let is_admin = row.try_get::<i64, _>("admin")? != 0;
        // On ne peut ni se rétrograder ni se supprimer soi-même : il resterait
        // possiblement un site sans administrateur.
        let actions = if Some(id) == me {
            String::new()
        } else {
            format!(
                "<form method=\"post\" action=\"{}\" class=\"d-inline\">{}\
                 <button type=\"submit\" class=\"btn btn-sm btn-outline-secondary\" title=\"{}\">{}</button></form> {}",
                ctx.url(&format!("admin/user/role/{id}")),
                csrf_field(&ctx),
                if is_admin { "Retirer les droits d'administrateur" } else { "Nommer administrateur" },
                text::icon(if is_admin { "fas fa-user-minus" } else { "fas fa-user-shield" }),
                delete_button(&ctx, &format!("admin/user/delete/{id}"), "Supprimer ce membre ?"),
            )
        };
        body.push_str(&format!(
            "<tr><td>{id}</td><td><a href=\"{}\">{username}</a></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td>\
             <td class=\"text-right text-nowrap\">{actions}</td></tr>",
            ctx.url(&format!("user/{id}/{}", text::url_title(&username))),
            text::clean(&row.try_get::<String, _>("email")?),
            badge(is_admin, "Administrateur", "Membre"),
            text::date_short(&registered),
            last.map(|d| text::date_short(&d)).unwrap_or_else(|| "—".to_owned()),
        ));
    }
    let content = format!(
        "<p><a class=\"btn btn-outline-secondary\" href=\"{}\">{} Gérer les groupes</a></p>\
         <div class=\"card\"><table class=\"table table-hover mb-0\"><thead><tr><th>#</th><th>Pseudo</th><th>Email</th>\
         <th>Groupe</th><th>Inscription</th><th>Dernière activité</th><th></th></tr></thead><tbody>{body}</tbody></table></div>",
        ctx.url("admin/user/groups"),
        text::icon("fas fa-users"),
    );
    render(&ctx, "user", "Membres", notice.message(), content)
}

/// Donne ou retire les droits d'administrateur.
pub async fn user_role(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    if let Some(denied) = deny(&ctx).await {
        return denied;
    }
    check_csrf(&ctx, &form.csrf)?;
    if ctx.user.as_ref().map(|u| u.id) == Some(id) {
        return Err(AppError::BadRequest("Vous ne pouvez pas modifier vos propres droits."));
    }
    sqlx::query("UPDATE nf_user SET admin = IF(admin = '1', '0', '1') WHERE id = ? AND deleted = '0'")
        .bind(id)
        .execute(&ctx.state.db)
        .await?;
    redirect(&ctx, "admin/user?ok=1")
}

/// Supprime un membre. Comme l'original, le compte est marqué `deleted` : ses
/// contenus restent, il ne peut plus se connecter et son pseudo redevient libre.
pub async fn user_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    if let Some(denied) = deny(&ctx).await {
        return denied;
    }
    check_csrf(&ctx, &form.csrf)?;
    if ctx.user.as_ref().map(|u| u.id) == Some(id) {
        return Err(AppError::BadRequest("Vous ne pouvez pas supprimer votre propre compte ici."));
    }
    let mut tx = ctx.state.db.begin().await?;
    sqlx::query("UPDATE nf_user SET deleted = '1', admin = '0' WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM nf_session WHERE user_id = ?").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    redirect(&ctx, "admin/user?ok=1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags() {
        assert_eq!(normalize_tags("a, b;c  a"), "a,b,c");
        assert_eq!(normalize_tags("  "), "");
    }
}
