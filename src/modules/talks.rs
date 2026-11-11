//! Module Discussions (tchat) : fils de messages courts, widget et administration.
//! Porté de `modules/talks` et `widgets/talks`.
//!
//! Permissions par discussion (`nf_access`, module `talks`) : `read`, `write`, `delete`.
//! La discussion n°1 (« Admin ») est réservée aux administrateurs et ne peut être ni
//! renommée ni supprimée, comme dans l'original.
//!
//! Différences avec l'original :
//! - les messages se rafraîchissent toutes les dix secondes (l'original interroge le
//!   serveur en continu) ; l'envoi recharge la page ;
//! - écrire demande d'être connecté (défaut de l'original : visiteurs refusés) ;
//! - sans règle détaillée, seuls l'auteur d'un message et les administrateurs peuvent
//!   le supprimer (défaut de l'original : administrateurs).

use super::admin::{self, admin_guard, Ctxt, CsrfOnly, Notice, Reply};
use crate::{
    core::{
        ctx::Ctx,
        form::{self, fits, input},
        layout::{self, Page},
        pagination::Pagination,
        panel::{Block, Panel},
        php::Value,
        text, throttle, user,
    },
    error::AppError,
};
use axum::{
    extract::{Path, Query},
    response::{IntoResponse, Redirect},
    Extension, Form,
};
use chrono::NaiveDateTime;
use serde::Deserialize;
use sqlx::Row;
use std::time::Duration;

pub const TITLE: &str = "Discussions";
pub const ICON: &str = "far fa-comment";
const ADMIN_TALK: u32 = 1;
const PER_PAGE: i64 = 30;
const MAX_MESSAGE: usize = 500;

struct Talk {
    talk_id: u32,
    name: String,
}

impl Talk {
    fn path(&self) -> String {
        format!("talks/{}/{}", self.talk_id, text::url_title(&self.name))
    }
}

async fn find(ctx: &Ctx, id: u32) -> Result<Option<Talk>, AppError> {
    let row = sqlx::query("SELECT talk_id, name FROM nf_talks WHERE talk_id = ?")
        .bind(id)
        .fetch_optional(&ctx.state.db)
        .await?;
    Ok(match row {
        Some(row) => Some(Talk { talk_id: row.try_get("talk_id")?, name: text::clean(&row.try_get::<String, _>("name")?) }),
        None => None,
    })
}

fn can_read(ctx: &Ctx, talk_id: u32) -> bool {
    if talk_id == ADMIN_TALK {
        return ctx.access.admin();
    }
    ctx.access.can("talks", "read", talk_id)
}

fn can_write(ctx: &Ctx, talk_id: u32) -> bool {
    ctx.user.is_some() && can_read(ctx, talk_id) && (talk_id == ADMIN_TALK || ctx.access.can("talks", "write", talk_id))
}

fn can_delete(ctx: &Ctx, talk_id: u32, author: Option<u32>) -> bool {
    let Some(me) = &ctx.user else {
        return false;
    };
    if ctx.access.admin() {
        return true;
    }
    if !can_read(ctx, talk_id) {
        return false;
    }
    author == Some(me.id) || (ctx.access.detailed("talks", "delete", talk_id) && ctx.access.can("talks", "delete", talk_id))
}

struct Message {
    message_id: u32,
    user_id: Option<u32>,
    username: Option<String>,
    message: String,
    date: NaiveDateTime,
}

async fn messages(ctx: &Ctx, talk_id: u32, limit: i64, offset: u32) -> Result<Vec<Message>, AppError> {
    let rows = sqlx::query(
        "SELECT m.message_id, m.user_id, u.username, m.message, CAST(m.date AS DATETIME) AS date \
         FROM nf_talks_messages m LEFT JOIN nf_user u ON u.id = m.user_id \
         WHERE m.talk_id = ? ORDER BY m.message_id DESC LIMIT ? OFFSET ?",
    )
    .bind(talk_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&ctx.state.db)
    .await?;
    let mut list = Vec::with_capacity(rows.len());
    for row in &rows {
        list.push(Message {
            message_id: row.try_get("message_id")?,
            user_id: row.try_get("user_id")?,
            username: row.try_get::<Option<String>, _>("username")?.map(|u| text::clean(&u)),
            message: text::clean(&row.try_get::<String, _>("message")?),
            date: row.try_get("date")?,
        });
    }
    Ok(list)
}

/// Chemin local sûr vers lequel revenir après une action (jamais une URL externe).
fn safe_back(back: &str) -> Option<&str> {
    let back = back.trim_matches('/');
    let ok = back.len() <= 200 && back.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.')) && !back.contains("..");
    ok.then_some(back)
}

fn messages_html(ctx: &Ctx, talk_id: u32, list: &[Message], back: &str) -> String {
    if list.is_empty() {
        return "<div class=\"text-center text-muted p-3\">Aucun message pour le moment</div>".to_owned();
    }
    let mut html = String::from("<ul class=\"list-group list-group-flush\">");
    // Du plus ancien au plus récent, comme une conversation.
    for m in list.iter().rev() {
        let author = match (m.user_id, &m.username) {
            (Some(id), Some(name)) => user::link(ctx.base(), id, name),
            _ => "<i>Visiteur</i>".to_owned(),
        };
        let delete = if can_delete(ctx, talk_id, m.user_id) {
            format!(
                "<form method=\"post\" action=\"{}\" class=\"float-right\">{}<input type=\"hidden\" name=\"back\" value=\"{}\" />\
                 <button type=\"submit\" class=\"btn btn-link btn-sm text-danger p-0\" title=\"Supprimer\">{}</button></form>",
                ctx.url(&format!("talks/delete/{}", m.message_id)),
                form::csrf_field(ctx),
                text::encode(back),
                text::icon("far fa-trash-alt"),
            )
        } else {
            String::new()
        };
        html.push_str(&format!(
            "<li class=\"list-group-item\">{delete}<b>{author}</b> <small class=\"text-muted\">{}</small><br />{}</li>",
            text::date_time(&m.date),
            text::linkify(&m.message),
        ));
    }
    html.push_str("</ul>");
    html
}

/// Liste des messages, rafraîchie périodiquement par un petit script (sans lui, la
/// liste reste simplement celle du chargement de la page).
fn live_messages(ctx: &Ctx, talk_id: u32, list: &[Message], back: &str, limit: i64) -> String {
    format!(
        "<div data-talk=\"{}\">{}</div><script>(function(){{var b=document.currentScript.previousElementSibling;\
         setInterval(function(){{if(document.hidden)return;fetch(b.dataset.talk,{{credentials:'same-origin'}})\
         .then(function(r){{return r.ok?r.text():null}}).then(function(h){{if(h!==null)b.innerHTML=h}}).catch(function(){{}})}},10000)}})();</script>",
        text::encode(&ctx.url(&format!("talks/messages/{talk_id}?n={limit}&back={back}"))),
        messages_html(ctx, talk_id, list, back),
    )
}

#[derive(Deserialize)]
pub struct FragmentQuery {
    #[serde(default)]
    n: i64,
    #[serde(default)]
    back: String,
}

/// `talks/messages/{id}` : fragment HTML des derniers messages (rafraîchissement).
pub async fn fragment(Extension(ctx): Ctxt, Path(id): Path<u32>, Query(query): Query<FragmentQuery>) -> Reply {
    if !ctx.enabled("talks") || !can_read(&ctx, id) || find(&ctx, id).await?.is_none() {
        return layout::not_found(&ctx).await;
    }
    let list = messages(&ctx, id, query.n.clamp(1, PER_PAGE), 0).await?;
    let back = safe_back(&query.back).unwrap_or("");
    Ok(axum::response::Html(messages_html(&ctx, id, &list, back)).into_response())
}

fn write_form(ctx: &Ctx, talk_id: u32, back: &str) -> String {
    if !can_write(ctx, talk_id) {
        return if ctx.user.is_none() {
            format!(
                "<div class=\"text-center\"><a href=\"{}\">Connectez-vous</a> pour participer.</div>",
                ctx.url("user/login")
            )
        } else {
            String::new()
        };
    }
    format!(
        "<form method=\"post\" action=\"{}\">{}<input type=\"hidden\" name=\"back\" value=\"{}\" />\
         <div class=\"input-group\"><input type=\"text\" class=\"form-control\" name=\"message\" maxlength=\"{MAX_MESSAGE}\" placeholder=\"Votre message…\" autocomplete=\"off\" required />\
         <div class=\"input-group-append\"><button type=\"submit\" class=\"btn btn-primary\">{}</button></div></div></form>",
        ctx.url(&format!("talks/add/{talk_id}")),
        form::csrf_field(ctx),
        text::encode(back),
        text::icon("fas fa-paper-plane"),
    )
}

/// Discussions lisibles par le visiteur courant.
async fn readable(ctx: &Ctx) -> Result<Vec<Talk>, AppError> {
    let rows = sqlx::query("SELECT talk_id, name FROM nf_talks ORDER BY talk_id")
        .fetch_all(&ctx.state.db)
        .await?;
    let mut talks = Vec::new();
    for row in &rows {
        let talk = Talk { talk_id: row.try_get("talk_id")?, name: text::clean(&row.try_get::<String, _>("name")?) };
        if can_read(ctx, talk.talk_id) {
            talks.push(talk);
        }
    }
    Ok(talks)
}

pub async fn index(Extension(ctx): Ctxt) -> Reply {
    if !ctx.enabled("talks") {
        return layout::not_found(&ctx).await;
    }
    let talks = readable(&ctx).await?;
    let body = if talks.is_empty() {
        "<div class=\"text-center\">Aucune discussion accessible</div>".to_owned()
    } else {
        let items: String = talks
            .iter()
            .map(|t| format!("<a class=\"list-group-item list-group-item-action\" href=\"{}\">{} {}</a>", ctx.url(&t.path()), text::icon(ICON), t.name))
            .collect();
        format!("<div class=\"list-group list-group-flush\">{items}</div>")
    };
    let page = Page::new("talks", TITLE, ICON).blocks(&[Block::Panel(Panel::new().heading(TITLE, ICON).body_raw(body))]);
    layout::render(&ctx, page).await
}

/// `talks/{id}/{nom}` et `talks/{id}/{nom}/page/{n}`.
pub async fn route(Extension(ctx): Ctxt, Path(rest): Path<String>) -> Reply {
    if !ctx.enabled("talks") {
        return layout::not_found(&ctx).await;
    }
    let parts: Vec<&str> = rest.trim_matches('/').split('/').collect();
    let (id, slug, page) = match parts.as_slice() {
        [id, slug] => (id, slug, 1),
        [id, slug, "page", n] => match n.parse::<u32>() {
            Ok(n) if n >= 1 => (id, slug, n),
            _ => return layout::not_found(&ctx).await,
        },
        _ => return layout::not_found(&ctx).await,
    };
    let Ok(id) = id.parse::<u32>() else {
        return layout::not_found(&ctx).await;
    };
    let talk = match find(&ctx, id).await? {
        // Discussion illisible : même réponse qu'une discussion inexistante.
        Some(talk) if text::url_title(&talk.name) == *slug && can_read(&ctx, id) => talk,
        _ => return layout::not_found(&ctx).await,
    };
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nf_talks_messages WHERE talk_id = ?")
        .bind(id)
        .fetch_one(&ctx.state.db)
        .await?;
    let pagination = Pagination::new(page, PER_PAGE, total);
    if pagination.out_of_range() {
        return layout::not_found(&ctx).await;
    }
    let list = messages(&ctx, id, PER_PAGE, pagination.offset()).await?;
    let path = talk.path();
    let mut blocks = vec![Block::Panel(
        Panel::new()
            .heading(talk.name.clone(), ICON)
            .body_raw(if page == 1 { live_messages(&ctx, id, &list, &path, PER_PAGE) } else { messages_html(&ctx, id, &list, &path) })
            .footer(write_form(&ctx, id, &path), false),
    )];
    if let Some(panel) = pagination.panel(&ctx.url(&path)) {
        blocks.push(Block::Panel(panel));
    }
    let page = Page::new("talks", TITLE, ICON)
        .title(talk.name.clone())
        .crumb(talk.name.clone(), path)
        .blocks(&blocks);
    layout::render(&ctx, page).await
}

#[derive(Deserialize)]
pub struct MessageForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    back: String,
}

fn go_back(ctx: &Ctx, back: &str, fallback: &str) -> Reply {
    Ok(Redirect::to(&ctx.url(safe_back(back).unwrap_or(fallback))).into_response())
}

pub async fn add(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<MessageForm>) -> Reply {
    if !ctx.enabled("talks") {
        return layout::not_found(&ctx).await;
    }
    admin::check_csrf(&ctx, &form.csrf)?;
    let Some(talk) = find(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let Some(author) = &ctx.user else {
        return layout::forbidden(&ctx).await;
    };
    if !can_write(&ctx, id) {
        return layout::forbidden(&ctx).await;
    }
    let message = form.message.split_whitespace().collect::<Vec<_>>().join(" ");
    let key = format!("talks:{}", author.id);
    let minute = Duration::from_secs(60);
    if !message.is_empty() && message.chars().count() <= MAX_MESSAGE && !throttle::blocked(&ctx.state, &key, 10, minute) {
        throttle::hit(&ctx.state, &key, minute);
        sqlx::query("INSERT INTO nf_talks_messages (talk_id, user_id, message) VALUES (?, ?, ?)")
            .bind(id)
            .bind(author.id)
            .bind(text::encode(&message))
            .execute(&ctx.state.db)
            .await?;
    }
    go_back(&ctx, &form.back, &talk.path())
}

#[derive(Deserialize)]
pub struct DeleteForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    back: String,
}

pub async fn delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<DeleteForm>) -> Reply {
    if !ctx.enabled("talks") {
        return layout::not_found(&ctx).await;
    }
    admin::check_csrf(&ctx, &form.csrf)?;
    let row = sqlx::query("SELECT talk_id, user_id FROM nf_talks_messages WHERE message_id = ?")
        .bind(id)
        .fetch_optional(&ctx.state.db)
        .await?;
    let Some(row) = row else {
        return layout::not_found(&ctx).await;
    };
    let talk_id: u32 = row.try_get("talk_id")?;
    let author: Option<u32> = row.try_get("user_id")?;
    if !can_delete(&ctx, talk_id, author) {
        return layout::forbidden(&ctx).await;
    }
    sqlx::query("DELETE FROM nf_talks_messages WHERE message_id = ?")
        .bind(id)
        .execute(&ctx.state.db)
        .await?;
    let fallback = match find(&ctx, talk_id).await? {
        Some(talk) => talk.path(),
        None => "talks".to_owned(),
    };
    go_back(&ctx, &form.back, &fallback)
}

/// Widget : derniers messages d'une discussion (réglage `talk_id`).
pub async fn widget(ctx: &Ctx, settings: &Value) -> Result<Vec<Block>, AppError> {
    let talk_id = settings.get("talk_id").and_then(Value::as_i64).and_then(|id| u32::try_from(id).ok()).unwrap_or(2);
    let Some(talk) = find(ctx, talk_id).await? else {
        return Ok(Vec::new());
    };
    if !can_read(ctx, talk_id) {
        return Ok(Vec::new());
    }
    let list = messages(ctx, talk_id, 10, 0).await?;
    let back = safe_back(&ctx.path).unwrap_or("").to_owned();
    Ok(vec![Block::Panel(
        Panel::new()
            .heading(talk.name.clone(), ICON)
            .body_raw(live_messages(ctx, talk_id, &list, &back, 10))
            .footer(
                format!(
                    "{}<div class=\"text-right mt-2\"><a href=\"{}\"><small>Tous les messages</small></a></div>",
                    write_form(ctx, talk_id, &back),
                    ctx.url(&talk.path())
                ),
                false,
            ),
    )])
}

// ---------------------------------------------------------------------------
// Administration

pub async fn admin_list(Extension(ctx): Ctxt, Query(notice): Query<Notice>) -> Reply {
    admin_guard!(ctx, "talks");
    let rows = sqlx::query(
        "SELECT t.talk_id, t.name, (SELECT COUNT(*) FROM nf_talks_messages m WHERE m.talk_id = t.talk_id) AS messages \
         FROM nf_talks t ORDER BY t.talk_id",
    )
    .fetch_all(&ctx.state.db)
    .await?;
    let mut body = String::new();
    for row in &rows {
        let talk = Talk { talk_id: row.try_get("talk_id")?, name: text::clean(&row.try_get::<String, _>("name")?) };
        let actions = if talk.talk_id == ADMIN_TALK {
            "<td class=\"text-right text-muted\"><small>Réservée aux administrateurs</small></td>".to_owned()
        } else {
            admin::row_actions(
                &ctx,
                &format!("admin/{}", talk.path()),
                &format!("admin/talks/delete/{}", talk.talk_id),
                "Supprimer cette discussion et ses messages ?",
            )
        };
        body.push_str(&format!(
            "<tr><td><a href=\"{}\">{}</a></td><td>{}</td>{actions}</tr>",
            ctx.url(&talk.path()),
            talk.name,
            row.try_get::<i64, _>("messages")?,
        ));
    }
    let content = admin::list_card(&ctx, TITLE, Some("admin/talks/add"), &["Nom", "Messages", ""], &body);
    admin::render(&ctx, "talks", TITLE, notice.message(), content)
}

#[derive(Deserialize)]
pub struct TalkForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    name: String,
}

fn talk_form(ctx: &Ctx, action: &str, name: &str, error: Option<&str>) -> Reply {
    let fields = input("Nom", "name", name, 50, true, "");
    let title = if action.ends_with("/add") { "Ajouter une discussion" } else { "Modifier la discussion" };
    admin::render(ctx, "talks", title, None, admin::form_card(ctx, action, error, fields, "admin/talks"))
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && fits(name, 50) && !text::url_title(name).is_empty()
}

pub async fn admin_add_get(Extension(ctx): Ctxt) -> Reply {
    admin_guard!(ctx, "talks");
    talk_form(&ctx, "admin/talks/add", "", None)
}

pub async fn admin_add_post(Extension(ctx): Ctxt, Form(form): Form<TalkForm>) -> Reply {
    admin_guard!(ctx, "talks");
    admin::check_csrf(&ctx, &form.csrf)?;
    let name = form.name.trim();
    if !valid_name(name) {
        return talk_form(&ctx, "admin/talks/add", name, Some("Le nom est obligatoire (50 caractères maximum)."));
    }
    let mut tx = ctx.state.db.begin().await?;
    let talk_id = sqlx::query("INSERT INTO nf_talks (name) VALUES (?)")
        .bind(text::encode(name))
        .execute(&mut *tx)
        .await?
        .last_insert_id();
    // Permissions initiales de l'original : lecture pour tous, écriture refusée aux
    // visiteurs, suppression réservée aux administrateurs.
    for (action, detail) in [("read", None), ("write", Some(("visitors", "0"))), ("delete", Some(("admins", "1")))] {
        let access_id = sqlx::query("INSERT INTO nf_access (id, module, action) VALUES (?, 'talks', ?)")
            .bind(talk_id)
            .bind(action)
            .execute(&mut *tx)
            .await?
            .last_insert_id();
        if let Some((entity, authorized)) = detail {
            sqlx::query("INSERT INTO nf_access_details (access_id, entity, type, authorized) VALUES (?, ?, 'group', ?)")
                .bind(access_id)
                .bind(entity)
                .bind(authorized)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    admin::redirect(&ctx, "admin/talks?ok=1")
}

pub async fn admin_edit_get(Extension(ctx): Ctxt, Path((id, _slug)): Path<(u32, String)>) -> Reply {
    admin_guard!(ctx, "talks");
    match find(&ctx, id).await? {
        Some(talk) if id != ADMIN_TALK => talk_form(&ctx, &format!("admin/{}", talk.path()), &text::decode(&talk.name), None),
        _ => layout::not_found(&ctx).await,
    }
}

pub async fn admin_edit_post(Extension(ctx): Ctxt, Path((id, _slug)): Path<(u32, String)>, Form(form): Form<TalkForm>) -> Reply {
    admin_guard!(ctx, "talks");
    admin::check_csrf(&ctx, &form.csrf)?;
    let talk = match find(&ctx, id).await? {
        Some(talk) if id != ADMIN_TALK => talk,
        _ => return layout::not_found(&ctx).await,
    };
    let name = form.name.trim();
    if !valid_name(name) {
        return talk_form(&ctx, &format!("admin/{}", talk.path()), name, Some("Le nom est obligatoire (50 caractères maximum)."));
    }
    sqlx::query("UPDATE nf_talks SET name = ? WHERE talk_id = ?")
        .bind(text::encode(name))
        .bind(id)
        .execute(&ctx.state.db)
        .await?;
    admin::redirect(&ctx, "admin/talks?ok=1")
}

pub async fn admin_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx, "talks");
    admin::check_csrf(&ctx, &form.csrf)?;
    if id == ADMIN_TALK {
        return layout::forbidden(&ctx).await;
    }
    // `nf_talks_messages` n'a pas de clé étrangère : nettoyage explicite.
    let mut tx = ctx.state.db.begin().await?;
    sqlx::query("DELETE FROM nf_talks_messages WHERE talk_id = ?").bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM nf_access WHERE module = 'talks' AND id = ?").bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM nf_talks WHERE talk_id = ?").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    admin::redirect(&ctx, "admin/talks?ok=1")
}

#[cfg(test)]
mod tests {
    use super::safe_back;

    #[test]
    fn back_is_always_a_local_path() {
        assert_eq!(safe_back("talks/2/publique"), Some("talks/2/publique"));
        assert_eq!(safe_back("/news/page/2/"), Some("news/page/2"));
        assert_eq!(safe_back(""), Some(""));
        for bad in ["https://evil.example", "//evil.example", "a/../b", "x?y=1", "a b", "\\\\evil"] {
            assert!(safe_back(bad).is_none() || !safe_back(bad).unwrap().contains(':'), "{bad}");
        }
        assert_eq!(safe_back("//evil.example"), Some("evil.example"));
        assert!(safe_back("https://evil.example").is_none());
        assert!(safe_back("a/../b").is_none());
    }
}
