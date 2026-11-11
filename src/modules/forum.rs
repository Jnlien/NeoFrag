//! Module Forum : catégories, forums, sous-forums, sujets et messages.
//! Porté de `modules/forum` et `widgets/forum`.
//!
//! Les droits se règlent par catégorie (`nf_access`, module `forum`) : lire, écrire,
//! et pour la modération : éditer, supprimer, mettre en annonce, verrouiller, déplacer.
//! L'état d'un sujet reprend le codage d'origine : `1` annonce, `-1` verrouillé,
//! `-2` annonce verrouillée, `0` normal.
//!
//! Non portés : sondages (la table existe mais l'original ne les utilisait pas) et
//! suivi d'un sujet par email.

use super::admin::{self, admin_guard, Ctxt, CsrfOnly, Notice, Reply};
use crate::{
    core::{
        bbcode,
        ctx::Ctx,
        form::{self, fits, input},
        layout::{self, Page},
        pagination::Pagination,
        panel::{Block, Panel},
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

pub const TITLE: &str = "Forum";
pub const ICON: &str = "fas fa-comments";
const TOPICS_PER_PAGE: i64 = 20;
const MESSAGES_PER_PAGE: i64 = 15;
const MAX_MESSAGE: usize = 20_000;

/// Droits créés avec une catégorie : (action, entité, autorisé). Mêmes valeurs par
/// défaut que la catégorie d'origine : lecture pour tous, écriture pour les membres,
/// modération pour les administrateurs.
const DEFAULT_ACCESS: [(&str, Option<(&str, bool)>); 7] = [
    ("category_read", None),
    ("category_write", Some(("visitors", false))),
    ("category_modify", Some(("admins", true))),
    ("category_delete", Some(("admins", true))),
    ("category_announce", Some(("admins", true))),
    ("category_lock", Some(("admins", true))),
    ("category_move", Some(("admins", true))),
];

fn can(ctx: &Ctx, action: &str, category_id: u32) -> bool {
    ctx.access.can("forum", action, category_id)
}

fn forum_path(forum_id: u32, title: &str) -> String {
    format!("forum/{forum_id}/{}", text::url_title(title))
}

fn topic_path(topic_id: u32, title: &str) -> String {
    format!("forum/topic/{topic_id}/{}", text::url_title(title))
}

/// Catégories que le visiteur peut lire, sous forme de liste SQL.
async fn readable_categories(ctx: &Ctx) -> Result<String, AppError> {
    let ids: Vec<u32> = sqlx::query_scalar("SELECT category_id FROM nf_forum_categories")
        .fetch_all(&ctx.state.db)
        .await?;
    let readable: Vec<String> = ids
        .into_iter()
        .filter(|id| can(ctx, "category_read", *id))
        .map(|id| id.to_string())
        .collect();
    Ok(if readable.is_empty() { "0".to_owned() } else { readable.join(",") })
}

/// Expression SQL de la catégorie d'un forum `f` (via son parent `pf` s'il est sous-forum).
const CATEGORY_OF: &str = "IF(f.is_subforum = '1', pf.parent_id, f.parent_id)";
const PARENT_JOIN: &str = "LEFT JOIN nf_forum pf ON f.is_subforum = '1' AND pf.forum_id = f.parent_id";

struct Forum {
    forum_id: u32,
    title: String,
    description: String,
    parent_id: u32,
    is_subforum: bool,
    category_id: u32,
    url: Option<String>,
    parent_title: Option<String>,
}

impl Forum {
    fn path(&self) -> String {
        forum_path(self.forum_id, &self.title)
    }
}

async fn find_forum(ctx: &Ctx, forum_id: u32) -> Result<Option<Forum>, AppError> {
    let row = sqlx::query(&format!(
        "SELECT f.forum_id, f.title, f.description, f.parent_id, (f.is_subforum = '1') AS sub, \
                CAST({CATEGORY_OF} AS SIGNED) AS category_id, u.url, pf.title AS parent_title \
         FROM nf_forum f {PARENT_JOIN} LEFT JOIN nf_forum_url u ON u.forum_id = f.forum_id WHERE f.forum_id = ?"
    ))
    .bind(forum_id)
    .fetch_optional(&ctx.state.db)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(Forum {
        forum_id: row.try_get("forum_id")?,
        title: text::clean(&row.try_get::<String, _>("title")?),
        description: text::clean(&row.try_get::<String, _>("description")?),
        parent_id: row.try_get("parent_id")?,
        is_subforum: row.try_get::<i64, _>("sub")? != 0,
        category_id: row.try_get::<Option<i64>, _>("category_id")?.unwrap_or(0).max(0) as u32,
        url: row.try_get("url")?,
        parent_title: row.try_get::<Option<String>, _>("parent_title")?.map(|t| text::clean(&t)),
    }))
}

struct Topic {
    topic_id: u32,
    forum_id: u32,
    first_message_id: Option<u32>,
    title: String,
    announce: bool,
    locked: bool,
    replies: u32,
    forum: Forum,
}

impl Topic {
    fn path(&self) -> String {
        topic_path(self.topic_id, &self.title)
    }

    fn status(announce: bool, locked: bool) -> &'static str {
        match (announce, locked) {
            (true, true) => "-2",
            (false, true) => "-1",
            (true, false) => "1",
            (false, false) => "0",
        }
    }
}

async fn find_topic(ctx: &Ctx, topic_id: u32) -> Result<Option<Topic>, AppError> {
    let row = sqlx::query(
        "SELECT topic_id, forum_id, message_id, title, (status IN ('-2', '1')) AS announce, \
                (status IN ('-2', '-1')) AS locked, CAST(count_messages AS SIGNED) AS count_messages \
         FROM nf_forum_topics WHERE topic_id = ?",
    )
    .bind(topic_id)
    .fetch_optional(&ctx.state.db)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let forum_id: u32 = row.try_get("forum_id")?;
    let Some(forum) = find_forum(ctx, forum_id).await? else {
        return Ok(None);
    };
    Ok(Some(Topic {
        topic_id: row.try_get("topic_id")?,
        forum_id,
        first_message_id: row.try_get("message_id")?,
        title: text::clean(&row.try_get::<String, _>("title")?),
        announce: row.try_get::<i64, _>("announce")? != 0,
        locked: row.try_get::<i64, _>("locked")? != 0,
        replies: row.try_get::<i64, _>("count_messages")?.max(0) as u32,
        forum,
    }))
}

/// Recalcule les compteurs et le dernier message d'un sujet puis de son forum. Appelé
/// après chaque écriture : plus sûr que des incréments, qui dérivent à la moindre erreur.
async fn recount(ctx: &Ctx, topic_id: Option<u32>, forum_id: u32) -> Result<(), AppError> {
    let db = &ctx.state.db;
    if let Some(topic_id) = topic_id {
        sqlx::query(
            "UPDATE nf_forum_topics t SET \
                t.count_messages = GREATEST((SELECT COUNT(*) FROM nf_forum_messages m WHERE m.topic_id = t.topic_id), 1) - 1, \
                t.last_message_id = (SELECT MAX(m.message_id) FROM nf_forum_messages m WHERE m.topic_id = t.topic_id) \
             WHERE t.topic_id = ?",
        )
        .bind(topic_id)
        .execute(db)
        .await?;
    }
    // Deux requêtes : MySQL refuse de lire dans une sous-requête la table qu'il met à jour.
    let row = sqlx::query(
        "SELECT COUNT(*) AS topics, CAST(IFNULL(SUM(count_messages), 0) AS SIGNED) AS messages, MAX(last_message_id) AS last \
         FROM nf_forum_topics WHERE forum_id = ?",
    )
    .bind(forum_id)
    .fetch_one(db)
    .await?;
    sqlx::query("UPDATE nf_forum SET count_topics = ?, count_messages = ?, last_message_id = ? WHERE forum_id = ?")
        .bind(row.try_get::<i64, _>("topics")?)
        .bind(row.try_get::<i64, _>("messages")?)
        .bind(row.try_get::<Option<u32>, _>("last")?)
        .bind(forum_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Message prêt à être enregistré, ou `None` s'il est vide ou trop long.
fn prepare(raw: &str) -> Option<String> {
    let message = raw.replace("\r\n", "\n");
    let message = message.trim();
    (!message.is_empty() && message.chars().count() <= MAX_MESSAGE).then(|| text::encode(message))
}

fn post_throttled(ctx: &Ctx, user_id: u32) -> bool {
    let key = format!("forum:{user_id}");
    let window = Duration::from_secs(5 * 60);
    if throttle::blocked(&ctx.state, &key, 10, window) {
        return true;
    }
    throttle::hit(&ctx.state, &key, window);
    false
}

fn last_message_html(ctx: &Ctx, row: &sqlx::mysql::MySqlRow) -> sqlx::Result<String> {
    let topic_id: Option<u32> = row.try_get("last_topic_id")?;
    let (Some(topic_id), Some(title), Some(date)) = (
        topic_id,
        row.try_get::<Option<String>, _>("last_title")?,
        row.try_get::<Option<NaiveDateTime>, _>("last_date")?,
    ) else {
        return Ok("<span class=\"text-muted\">Aucun message</span>".to_owned());
    };
    let title = text::clean(&title);
    let author = match (row.try_get::<Option<u32>, _>("last_user_id")?, row.try_get::<Option<String>, _>("last_username")?) {
        (Some(id), Some(name)) => user::link(ctx.base(), id, &name),
        _ => "Visiteur".to_owned(),
    };
    Ok(format!(
        "<a href=\"{}\">{}</a><br /><small class=\"text-muted\">{author} — {}</small>",
        ctx.url(&topic_path(topic_id, &title)),
        text::shorten(&title, 40),
        text::date_time(&date)
    ))
}

/// Colonnes et jointures du « dernier message » d'un forum `f`.
const LAST_COLUMNS: &str = "lt.topic_id AS last_topic_id, lt.title AS last_title, CAST(lm.date AS DATETIME) AS last_date, \
     lu.id AS last_user_id, lu.username AS last_username";
const LAST_JOINS: &str = "LEFT JOIN nf_forum_messages lm ON lm.message_id = f.last_message_id \
     LEFT JOIN nf_forum_topics lt ON lt.topic_id = lm.topic_id \
     LEFT JOIN nf_user lu ON lu.id = lm.user_id AND lu.deleted = '0'";

/// Lignes de tableau pour une liste de forums (accueil, sous-forums).
async fn forum_rows(ctx: &Ctx, condition: &str, parent_id: u32) -> Result<String, AppError> {
    let me = ctx.user.as_ref().map(|u| u.id).unwrap_or(0);
    let rows = sqlx::query(&format!(
        "SELECT f.forum_id, f.title, f.description, CAST(f.count_topics AS SIGNED) AS count_topics, \
                CAST(f.count_messages AS SIGNED) AS count_messages, u.url, CAST(u.redirects AS SIGNED) AS redirects, {LAST_COLUMNS}, \
                EXISTS(SELECT 1 FROM nf_forum_topics t INNER JOIN nf_forum_messages m ON m.message_id = t.last_message_id \
                       LEFT JOIN nf_forum_topics_read tr ON tr.topic_id = t.topic_id AND tr.user_id = ? \
                       LEFT JOIN nf_forum_read fr ON fr.forum_id = t.forum_id AND fr.user_id = ? \
                       WHERE t.forum_id = f.forum_id AND m.date > GREATEST(IFNULL(tr.date, '1970-01-02'), IFNULL(fr.date, '1970-01-02'))) AS unread \
         FROM nf_forum f LEFT JOIN nf_forum_url u ON u.forum_id = f.forum_id {LAST_JOINS} \
         WHERE {condition} AND f.parent_id = ? ORDER BY f.`order`, f.forum_id"
    ))
    .bind(me)
    .bind(me)
    .bind(parent_id)
    .fetch_all(&ctx.state.db)
    .await?;

    let mut html = String::new();
    for row in &rows {
        let forum_id: u32 = row.try_get("forum_id")?;
        let title = text::clean(&row.try_get::<String, _>("title")?);
        let description = text::clean(&row.try_get::<String, _>("description")?);
        let is_link = row.try_get::<Option<String>, _>("url")?.is_some();
        let unread = me != 0 && row.try_get::<i64, _>("unread")? != 0;
        let icon = if is_link {
            text::icon("fas fa-globe")
        } else if unread {
            format!("<span class=\"text-primary\">{}</span>", text::icon("fas fa-comments"))
        } else {
            format!("<span class=\"text-muted\">{}</span>", text::icon("far fa-comments"))
        };
        let (stats, last) = if is_link {
            (
                format!("{} redirection(s)", row.try_get::<Option<i64>, _>("redirects")?.unwrap_or(0)),
                String::new(),
            )
        } else {
            (
                format!(
                    "{} sujet(s)<br />{} réponse(s)",
                    row.try_get::<i64, _>("count_topics")?,
                    row.try_get::<i64, _>("count_messages")?
                ),
                last_message_html(ctx, row)?,
            )
        };
        html.push_str(&format!(
            "<tr><td class=\"align-middle text-center\" style=\"width: 50px; font-size: 20px;\">{icon}</td>\
             <td><a href=\"{}\"><b>{title}</b></a><br /><small class=\"text-muted\">{description}</small></td>\
             <td class=\"align-middle text-nowrap\"><small>{stats}</small></td><td class=\"align-middle\">{last}</td></tr>",
            ctx.url(&forum_path(forum_id, &title)),
        ));
    }
    Ok(html)
}

fn forum_table(rows: &str) -> String {
    format!("<div class=\"table-responsive\"><table class=\"table mb-0\"><tbody>{rows}</tbody></table></div>")
}

async fn index_page(ctx: &Ctx) -> Reply {
    let categories = sqlx::query("SELECT category_id, title FROM nf_forum_categories ORDER BY `order`, category_id")
        .fetch_all(&ctx.state.db)
        .await?;
    let mut blocks = Vec::new();
    for row in &categories {
        let category_id: u32 = row.try_get("category_id")?;
        if !can(ctx, "category_read", category_id) {
            continue;
        }
        let rows = forum_rows(ctx, "f.is_subforum = '0'", category_id).await?;
        if rows.is_empty() {
            continue;
        }
        blocks.push(Block::Panel(
            Panel::new()
                .heading(text::clean(&row.try_get::<String, _>("title")?), ICON)
                .body_raw(forum_table(&rows)),
        ));
    }
    if blocks.is_empty() {
        blocks.push(Block::Panel(
            Panel::new()
                .heading(TITLE, ICON)
                .style("card-info")
                .body("<div class=\"text-center\">Aucun forum</div>"),
        ));
    } else if ctx.user.is_some() {
        blocks.push(Block::Html(format!(
            "<form method=\"post\" action=\"{}\" class=\"text-right\">{}<button type=\"submit\" class=\"btn btn-light\">{} Marquer tous les messages comme lus</button></form>",
            ctx.url("forum/mark-all-as-read"),
            form::csrf_field(ctx),
            text::icon("far fa-eye")
        )));
    }
    layout::render(ctx, Page::new("forum", TITLE, ICON).blocks(&blocks)).await
}

async fn forum_page(ctx: &Ctx, forum_id: u32, slug: &str, page_no: u32) -> Reply {
    let forum = match find_forum(ctx, forum_id).await? {
        Some(forum) if text::url_title(&forum.title) == slug => forum,
        _ => return layout::not_found(ctx).await,
    };
    if !can(ctx, "category_read", forum.category_id) {
        return layout::forbidden(ctx).await;
    }
    let db = &ctx.state.db;
    if let Some(url) = &forum.url {
        // Forum « lien » : compte la redirection puis renvoie vers l'adresse.
        let url = text::decode(url);
        if url.starts_with("https://") || url.starts_with("http://") {
            sqlx::query("UPDATE nf_forum_url SET redirects = redirects + 1 WHERE forum_id = ?")
                .bind(forum_id)
                .execute(db)
                .await?;
            return Ok(Redirect::to(&url).into_response());
        }
        return layout::not_found(ctx).await;
    }

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nf_forum_topics WHERE forum_id = ?")
        .bind(forum_id)
        .fetch_one(db)
        .await?;
    let pagination = Pagination::new(page_no, TOPICS_PER_PAGE, total);
    if pagination.out_of_range() {
        return Ok(Redirect::to(&ctx.url(&forum.path())).into_response());
    }
    let me = ctx.user.as_ref().map(|u| u.id).unwrap_or(0);
    let topics = sqlx::query(
        "SELECT t.topic_id, t.title, CAST(t.views AS SIGNED) AS views, CAST(t.count_messages AS SIGNED) AS count_messages, \
                (t.status IN ('-2', '1')) AS announce, \
                (t.status IN ('-2', '-1')) AS locked, u.id AS user_id, u.username, \
                CAST(lm.date AS DATETIME) AS last_date, lu.id AS last_user_id, lu.username AS last_username, \
                (lm.date > GREATEST(IFNULL(tr.date, '1970-01-02'), IFNULL(fr.date, '1970-01-02'))) AS unread \
         FROM nf_forum_topics t \
         LEFT JOIN nf_forum_messages fm ON fm.message_id = t.message_id \
         LEFT JOIN nf_user u ON u.id = fm.user_id AND u.deleted = '0' \
         LEFT JOIN nf_forum_messages lm ON lm.message_id = t.last_message_id \
         LEFT JOIN nf_user lu ON lu.id = lm.user_id AND lu.deleted = '0' \
         LEFT JOIN nf_forum_topics_read tr ON tr.topic_id = t.topic_id AND tr.user_id = ? \
         LEFT JOIN nf_forum_read fr ON fr.forum_id = t.forum_id AND fr.user_id = ? \
         WHERE t.forum_id = ? ORDER BY (t.status IN ('-2', '1')) DESC, t.last_message_id DESC LIMIT ? OFFSET ?",
    )
    .bind(me)
    .bind(me)
    .bind(forum_id)
    .bind(pagination.per_page)
    .bind(pagination.offset())
    .fetch_all(db)
    .await?;

    let mut rows = String::new();
    for row in &topics {
        let topic_id: u32 = row.try_get("topic_id")?;
        let title = text::clean(&row.try_get::<String, _>("title")?);
        let announce = row.try_get::<i64, _>("announce")? != 0;
        let locked = row.try_get::<i64, _>("locked")? != 0;
        let unread = me != 0 && row.try_get::<Option<i64>, _>("unread")?.unwrap_or(0) != 0;
        let author = match (row.try_get::<Option<u32>, _>("user_id")?, row.try_get::<Option<String>, _>("username")?) {
            (Some(id), Some(name)) => user::link(ctx.base(), id, &name),
            _ => "Visiteur".to_owned(),
        };
        let last = match row.try_get::<Option<NaiveDateTime>, _>("last_date")? {
            Some(date) => {
                let who = match (row.try_get::<Option<u32>, _>("last_user_id")?, row.try_get::<Option<String>, _>("last_username")?) {
                    (Some(id), Some(name)) => user::link(ctx.base(), id, &name),
                    _ => "Visiteur".to_owned(),
                };
                format!("{who}<br /><small class=\"text-muted\">{}</small>", text::date_time(&date))
            }
            None => String::new(),
        };
        rows.push_str(&format!(
            "<tr><td class=\"align-middle text-center\" style=\"width: 50px; font-size: 18px;\">{}</td>\
             <td>{}{}<a href=\"{}\">{}{title}{}</a><br /><small class=\"text-muted\">Par {author}</small></td>\
             <td class=\"align-middle text-nowrap\"><small>{} réponse(s)<br />{} vue(s)</small></td><td class=\"align-middle\">{last}</td></tr>",
            if unread {
                format!("<span class=\"text-primary\">{}</span>", text::icon("fas fa-comment"))
            } else {
                format!("<span class=\"text-muted\">{}</span>", text::icon("far fa-comment"))
            },
            if announce { "<span class=\"badge badge-warning mr-1\">Annonce</span>" } else { "" },
            if locked { format!("{} ", text::icon("fas fa-lock")) } else { String::new() },
            ctx.url(&topic_path(topic_id, &title)),
            if unread { "<b>" } else { "" },
            if unread { "</b>" } else { "" },
            row.try_get::<i64, _>("count_messages")?,
            row.try_get::<i64, _>("views")?,
        ));
    }

    let mut blocks = Vec::new();
    if !forum.is_subforum {
        let subforums = forum_rows(ctx, "f.is_subforum = '1'", forum_id).await?;
        if !subforums.is_empty() {
            blocks.push(Block::Panel(Panel::new().heading("Sous-forums", ICON).body_raw(forum_table(&subforums))));
        }
    }
    if can(ctx, "category_write", forum.category_id) {
        blocks.push(Block::Html(format!(
            "<p class=\"text-right\"><a class=\"btn btn-primary\" href=\"{}\">{} Nouveau sujet</a></p>",
            ctx.url(&format!("forum/new/{forum_id}/{slug}")),
            text::icon("fas fa-plus")
        )));
    }
    let panel = Panel::new().heading(forum.title.clone(), ICON);
    blocks.push(Block::Panel(if rows.is_empty() {
        panel.body("<div class=\"text-center\">Aucun sujet pour le moment</div>")
    } else {
        panel.body_raw(forum_table(&rows))
    }));
    if let Some(panel) = pagination.panel(&ctx.url(&forum.path())) {
        blocks.push(Block::Panel(panel));
    }

    let mut page = Page::new("forum", TITLE, ICON).method("_forum").title(forum.title.clone());
    if let (true, Some(parent)) = (forum.is_subforum, &forum.parent_title) {
        page = page.crumb(parent.clone(), forum_path(forum.parent_id, parent));
    }
    layout::render(ctx, page.crumb(forum.title.clone(), forum.path()).blocks(&blocks)).await
}

fn message_field(value: &str, rows: u8) -> String {
    format!(
        "<div class=\"form-group\"><label for=\"f-message\"><b>Message</b> <small class=\"text-muted\">BBCode accepté</small></label>\
         <textarea class=\"form-control\" id=\"f-message\" name=\"message\" rows=\"{rows}\" maxlength=\"{MAX_MESSAGE}\" required>{}</textarea></div>",
        text::encode(value)
    )
}

async fn new_topic_page(ctx: &Ctx, forum: &Forum, title: &str, message: &str, error: Option<&str>) -> Reply {
    let body = format!(
        "{}<form method=\"post\" action=\"{}\">{}{}{}<button type=\"submit\" class=\"btn btn-primary\">Créer le sujet</button> \
         <a class=\"btn btn-link\" href=\"{}\">Annuler</a></form>",
        form::alert("danger", error),
        ctx.url(&format!("forum/new/{}", forum.forum_id)),
        form::csrf_field(ctx),
        input("Titre", "title", title, 100, true, ""),
        message_field(message, 12),
        ctx.url(&forum.path()),
    );
    let page = Page::new("forum", TITLE, ICON)
        .title("Nouveau sujet")
        .crumb(forum.title.clone(), forum.path())
        .crumb("Nouveau sujet", format!("forum/new/{}/x", forum.forum_id))
        .blocks(&[Block::Panel(Panel::new().heading("Nouveau sujet", "fas fa-plus").body(body))]);
    layout::render(ctx, page).await
}

/// Forum dans lequel le membre courant peut écrire, sinon la réponse d'erreur.
async fn writable_forum(ctx: &Ctx, forum_id: u32) -> Result<Result<Forum, Reply>, AppError> {
    let Some(forum) = find_forum(ctx, forum_id).await? else {
        return Ok(Err(layout::not_found(ctx).await));
    };
    if forum.url.is_some() || !can(ctx, "category_read", forum.category_id) || !can(ctx, "category_write", forum.category_id) {
        return Ok(Err(layout::forbidden(ctx).await));
    }
    Ok(Ok(forum))
}

#[derive(Deserialize)]
pub struct TopicForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    message: String,
}

pub async fn new_topic(Extension(ctx): Ctxt, Path(forum_id): Path<u32>, Form(form): Form<TopicForm>) -> Reply {
    if !ctx.enabled("forum") {
        return layout::not_found(&ctx).await;
    }
    let forum = match writable_forum(&ctx, forum_id).await? {
        Ok(forum) => forum,
        Err(reply) => return reply,
    };
    let Some(author) = &ctx.user else {
        return layout::forbidden(&ctx).await;
    };
    let title = form.title.trim();
    if !ctx.csrf_ok(&form.csrf) {
        return new_topic_page(&ctx, &forum, title, &form.message, Some("Votre session a expiré, veuillez réessayer.")).await;
    }
    if title.is_empty() || text::url_title(title).is_empty() || !fits(title, 100) {
        return new_topic_page(&ctx, &forum, title, &form.message, Some("Le titre est obligatoire (100 caractères maximum).")).await;
    }
    let Some(message) = prepare(&form.message) else {
        return new_topic_page(&ctx, &forum, title, &form.message, Some("Le message est vide ou trop long.")).await;
    };
    if post_throttled(&ctx, author.id) {
        return new_topic_page(&ctx, &forum, title, &form.message, Some("Trop de messages en peu de temps, patientez quelques minutes.")).await;
    }

    let mut tx = ctx.state.db.begin().await?;
    let topic_id = sqlx::query("INSERT INTO nf_forum_topics (forum_id, title, status, views, count_messages) VALUES (?, ?, '0', 0, 0)")
        .bind(forum_id)
        .bind(text::encode(title))
        .execute(&mut *tx)
        .await?
        .last_insert_id() as u32;
    let message_id = sqlx::query("INSERT INTO nf_forum_messages (topic_id, user_id, message) VALUES (?, ?, ?)")
        .bind(topic_id)
        .bind(author.id)
        .bind(message)
        .execute(&mut *tx)
        .await?
        .last_insert_id();
    sqlx::query("UPDATE nf_forum_topics SET message_id = ?, last_message_id = ? WHERE topic_id = ?")
        .bind(message_id)
        .bind(message_id)
        .bind(topic_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    recount(&ctx, Some(topic_id), forum_id).await?;
    Ok(Redirect::to(&ctx.url(&topic_path(topic_id, &text::encode(title)))).into_response())
}

async fn mark_topic_read(ctx: &Ctx, topic_id: u32) -> Result<(), AppError> {
    if let Some(user) = &ctx.user {
        sqlx::query(
            "INSERT INTO nf_forum_topics_read (topic_id, user_id, date) VALUES (?, ?, NOW()) ON DUPLICATE KEY UPDATE date = NOW()",
        )
        .bind(topic_id)
        .bind(user.id)
        .execute(&ctx.state.db)
        .await?;
    }
    Ok(())
}

async fn topic_page(ctx: &Ctx, topic_id: u32, slug: &str, page_no: u32) -> Reply {
    let topic = match find_topic(ctx, topic_id).await? {
        Some(topic) if text::url_title(&topic.title) == slug => topic,
        _ => return layout::not_found(ctx).await,
    };
    let category = topic.forum.category_id;
    if !can(ctx, "category_read", category) {
        return layout::forbidden(ctx).await;
    }
    let db = &ctx.state.db;
    let total = topic.replies as i64 + 1;
    let pagination = Pagination::new(page_no, MESSAGES_PER_PAGE, total);
    if pagination.out_of_range() {
        return Ok(Redirect::to(&ctx.url(&topic.path())).into_response());
    }
    sqlx::query("UPDATE nf_forum_topics SET views = views + 1 WHERE topic_id = ?")
        .bind(topic_id)
        .execute(db)
        .await?;
    mark_topic_read(ctx, topic_id).await?;

    let messages = sqlx::query(
        "SELECT m.message_id, m.user_id, m.message, CAST(m.date AS DATETIME) AS date, u.username, (u.admin = '1') AS admin, \
                f.path AS avatar, up.sex, IFNULL(up.signature, '') AS signature, \
                (SELECT COUNT(*) FROM nf_forum_messages m2 WHERE m2.user_id = m.user_id) AS posts \
         FROM nf_forum_messages m \
         LEFT JOIN nf_user u ON u.id = m.user_id AND u.deleted = '0' \
         LEFT JOIN nf_user_profile up ON up.id = u.id LEFT JOIN nf_file f ON f.id = up.avatar \
         WHERE m.topic_id = ? ORDER BY m.message_id LIMIT ? OFFSET ?",
    )
    .bind(topic_id)
    .bind(pagination.per_page)
    .bind(pagination.offset())
    .fetch_all(db)
    .await?;

    let me = ctx.user.as_ref().map(|u| u.id);
    let moderator = |action: &str| can(ctx, action, category);
    let mut blocks = Vec::new();

    // Outils de modération du sujet.
    let mut tools = String::new();
    let tool = |path: String, icon: &str, label: &str, confirm: &str| {
        format!(
            "<form method=\"post\" action=\"{}\" class=\"d-inline\"{}>{}<button type=\"submit\" class=\"btn btn-light btn-sm\">{} {label}</button></form> ",
            ctx.url(&path),
            if confirm.is_empty() { String::new() } else { format!(" onsubmit=\"return confirm('{}');\"", text::encode(confirm)) },
            form::csrf_field(ctx),
            text::icon(icon)
        )
    };
    if moderator("category_announce") {
        tools.push_str(&tool(
            format!("forum/moderate/announce/{topic_id}"),
            "fas fa-flag",
            if topic.announce { "Retirer des annonces" } else { "Mettre en annonce" },
            "",
        ));
    }
    if moderator("category_lock") {
        tools.push_str(&tool(
            format!("forum/moderate/lock/{topic_id}"),
            if topic.locked { "fas fa-unlock" } else { "fas fa-lock" },
            if topic.locked { "Déverrouiller" } else { "Verrouiller" },
            "",
        ));
    }
    if moderator("category_delete") {
        tools.push_str(&tool(
            format!("forum/moderate/delete/{topic_id}"),
            "far fa-trash-alt",
            "Supprimer le sujet",
            "Supprimer ce sujet et tous ses messages ?",
        ));
    }
    if moderator("category_move") {
        let forums = sqlx::query("SELECT f.forum_id, f.title FROM nf_forum f LEFT JOIN nf_forum_url u ON u.forum_id = f.forum_id WHERE u.forum_id IS NULL AND f.forum_id <> ? ORDER BY f.title")
            .bind(topic.forum_id)
            .fetch_all(db)
            .await?;
        if !forums.is_empty() {
            let mut options = String::new();
            for row in &forums {
                options.push_str(&format!(
                    "<option value=\"{}\">{}</option>",
                    row.try_get::<u32, _>("forum_id")?,
                    text::clean(&row.try_get::<String, _>("title")?)
                ));
            }
            tools.push_str(&format!(
                "<form method=\"post\" action=\"{}\" class=\"d-inline form-inline\">{}<select class=\"form-control form-control-sm mr-1\" name=\"forum_id\">{options}</select>\
                 <button type=\"submit\" class=\"btn btn-light btn-sm\">{} Déplacer</button></form>",
                ctx.url(&format!("forum/moderate/move/{topic_id}")),
                form::csrf_field(ctx),
                text::icon("fas fa-reply")
            ));
        }
    }
    blocks.push(Block::Panel(Panel::new().style("mb-4").body(format!(
        "<h4 class=\"mb-0\">{}{}{}</h4>{}",
        if topic.announce { "<span class=\"badge badge-warning mr-2\">Annonce</span>" } else { "" },
        if topic.locked { format!("{} ", text::icon("fas fa-lock")) } else { String::new() },
        topic.title,
        if tools.is_empty() { String::new() } else { format!("<div class=\"mt-3\">{tools}</div>") }
    ))));

    for row in &messages {
        let message_id: u32 = row.try_get("message_id")?;
        let author_id: Option<u32> = row.try_get("user_id")?;
        let username: Option<String> = row.try_get("username")?;
        let avatar: Option<String> = row.try_get("avatar")?;
        let sex: Option<String> = row.try_get("sex")?;
        let date: NaiveDateTime = row.try_get("date")?;
        let message = row.try_get::<Option<String>, _>("message")?.map(|m| text::clean(&m)).unwrap_or_default();
        let signature = text::clean(&row.try_get::<String, _>("signature")?);
        let is_first = Some(message_id) == topic.first_message_id;
        let mine = me.is_some() && me == author_id;

        let author = match (&username, author_id) {
            (Some(name), Some(id)) => format!(
                "{}<h6 class=\"mt-2 mb-0\">{}</h6>{}<br /><small class=\"text-muted\">{} message(s)</small>",
                user::avatar_html(ctx.base(), avatar.as_deref(), sex.as_deref(), false, "m-auto"),
                user::link(ctx.base(), id, name),
                if row.try_get::<Option<i64>, _>("admin")?.unwrap_or(0) != 0 {
                    "<span class=\"badge badge-danger\">Administrateur</span>"
                } else {
                    "<span class=\"badge badge-success\">Membre</span>"
                },
                row.try_get::<i64, _>("posts")?,
            ),
            _ => "<h6>Visiteur</h6>".to_owned(),
        };
        let mut actions = String::new();
        if moderator("category_modify") || (mine && !topic.locked) {
            actions.push_str(&format!(
                "<a class=\"btn btn-link btn-sm\" href=\"{}\">{} Éditer</a>",
                ctx.url(&format!("forum/message/edit/{message_id}/{slug}")),
                text::icon("fas fa-edit")
            ));
        }
        if moderator("category_delete") && !is_first {
            actions.push_str(&format!(
                "<form method=\"post\" action=\"{}\" class=\"d-inline\" onsubmit=\"return confirm('Supprimer ce message ?');\">{}\
                 <button type=\"submit\" class=\"btn btn-link btn-sm text-danger\">{} Supprimer</button></form>",
                ctx.url(&format!("forum/message/delete/{message_id}")),
                form::csrf_field(ctx),
                text::icon("far fa-trash-alt")
            ));
        }
        blocks.push(Block::Panel(Panel::new().body_raw(format!(
            "<div class=\"card-body\" id=\"message-{message_id}\"><div class=\"row\"><div class=\"col-12 col-md-3 col-lg-2 text-center border-right\">{author}</div>\
             <div class=\"col-12 col-md-9 col-lg-10\"><div class=\"float-right\">{actions}</div>\
             <small class=\"text-muted\">{} {}</small><hr class=\"mt-2\" />{}{}</div></div></div>",
            text::icon("far fa-clock"),
            text::date_time(&date),
            bbcode::render(&message),
            if signature.is_empty() { String::new() } else { format!("<hr /><small class=\"text-muted\">{}</small>", bbcode::render(&signature)) },
        ))));
    }
    if let Some(panel) = pagination.panel(&ctx.url(&topic.path())) {
        blocks.push(Block::Panel(panel));
    }

    if topic.locked && !moderator("category_lock") {
        blocks.push(Block::Html(format!(
            "<div class=\"alert alert-warning\">{} Ce sujet est verrouillé</div>",
            text::icon("fas fa-lock")
        )));
    } else if can(ctx, "category_write", category) {
        blocks.push(Block::Panel(Panel::new().heading("Répondre", "fas fa-reply").body(format!(
            "<form method=\"post\" action=\"{}\">{}{}<button type=\"submit\" class=\"btn btn-primary\">Envoyer</button></form>",
            ctx.url(&format!("forum/reply/{topic_id}")),
            form::csrf_field(ctx),
            message_field("", 6),
        ))));
    } else if ctx.user.is_none() {
        blocks.push(Block::Html(format!(
            "<div class=\"alert alert-info\">Vous devez être <a href=\"{}\">identifié</a> pour répondre.</div>",
            ctx.url("user/login")
        )));
    }

    let page = Page::new("forum", TITLE, ICON)
        .method("_topic")
        .title(topic.title.clone())
        .crumb(topic.forum.title.clone(), topic.forum.path())
        .crumb(topic.title.clone(), topic.path())
        .blocks(&blocks);
    layout::render(ctx, page).await
}

#[derive(Deserialize)]
pub struct MessageForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    message: String,
}

pub async fn reply(Extension(ctx): Ctxt, Path(topic_id): Path<u32>, Form(form): Form<MessageForm>) -> Reply {
    if !ctx.enabled("forum") {
        return layout::not_found(&ctx).await;
    }
    let Some(topic) = find_topic(&ctx, topic_id).await? else {
        return layout::not_found(&ctx).await;
    };
    let category = topic.forum.category_id;
    let Some(author) = &ctx.user else {
        return layout::forbidden(&ctx).await;
    };
    let allowed = can(&ctx, "category_read", category)
        && can(&ctx, "category_write", category)
        && (!topic.locked || can(&ctx, "category_lock", category));
    if !allowed {
        return layout::forbidden(&ctx).await;
    }
    let back = ctx.url(&topic.path());
    if !ctx.csrf_ok(&form.csrf) {
        return Ok(Redirect::to(&back).into_response());
    }
    let Some(message) = prepare(&form.message) else {
        return Err(AppError::BadRequest("Le message est vide ou trop long."));
    };
    if post_throttled(&ctx, author.id) {
        return Err(AppError::BadRequest("Trop de messages en peu de temps, patientez quelques minutes."));
    }
    let message_id = sqlx::query("INSERT INTO nf_forum_messages (topic_id, user_id, message) VALUES (?, ?, ?)")
        .bind(topic_id)
        .bind(author.id)
        .bind(message)
        .execute(&ctx.state.db)
        .await?
        .last_insert_id();
    recount(&ctx, Some(topic_id), topic.forum_id).await?;
    mark_topic_read(&ctx, topic_id).await?;

    // Vers la dernière page, sur le nouveau message.
    let pages = Pagination::new(1, MESSAGES_PER_PAGE, topic.replies as i64 + 2).pages();
    let suffix = if pages > 1 { format!("/page/{pages}") } else { String::new() };
    Ok(Redirect::to(&format!("{back}{suffix}#message-{message_id}")).into_response())
}

struct Message {
    message_id: u32,
    author_id: Option<u32>,
    text: String,
    topic: Topic,
}

async fn find_message(ctx: &Ctx, message_id: u32) -> Result<Option<Message>, AppError> {
    let row = sqlx::query("SELECT message_id, topic_id, user_id, message FROM nf_forum_messages WHERE message_id = ?")
        .bind(message_id)
        .fetch_optional(&ctx.state.db)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let Some(topic) = find_topic(ctx, row.try_get("topic_id")?).await? else {
        return Ok(None);
    };
    Ok(Some(Message {
        message_id,
        author_id: row.try_get("user_id")?,
        text: row.try_get::<Option<String>, _>("message")?.unwrap_or_default(),
        topic,
    }))
}

fn can_edit(ctx: &Ctx, message: &Message) -> bool {
    let category = message.topic.forum.category_id;
    let mine = ctx.user.as_ref().is_some_and(|u| Some(u.id) == message.author_id);
    can(ctx, "category_read", category) && (can(ctx, "category_modify", category) || (mine && !message.topic.locked))
}

async fn edit_page(ctx: &Ctx, message: &Message, title: &str, body: &str, error: Option<&str>) -> Reply {
    let is_first = Some(message.message_id) == message.topic.first_message_id;
    let html = format!(
        "{}<form method=\"post\" action=\"{}\">{}{}{}<button type=\"submit\" class=\"btn btn-primary\">Enregistrer</button> \
         <a class=\"btn btn-link\" href=\"{}\">Annuler</a></form>",
        form::alert("danger", error),
        ctx.url(&format!("forum/message/edit/{}", message.message_id)),
        form::csrf_field(ctx),
        if is_first { input("Titre du sujet", "title", title, 100, true, "") } else { String::new() },
        message_field(body, 12),
        ctx.url(&message.topic.path()),
    );
    let page = Page::new("forum", TITLE, ICON)
        .title("Éditer le message")
        .crumb(message.topic.title.clone(), message.topic.path())
        .blocks(&[Block::Panel(Panel::new().heading("Éditer le message", "fas fa-edit").body(html))]);
    layout::render(ctx, page).await
}

pub async fn edit_message(Extension(ctx): Ctxt, Path(message_id): Path<u32>, Form(form): Form<MessageForm>) -> Reply {
    if !ctx.enabled("forum") {
        return layout::not_found(&ctx).await;
    }
    let Some(message) = find_message(&ctx, message_id).await? else {
        return layout::not_found(&ctx).await;
    };
    if !can_edit(&ctx, &message) {
        return layout::forbidden(&ctx).await;
    }
    let is_first = Some(message_id) == message.topic.first_message_id;
    let title = form.title.trim();
    if !ctx.csrf_ok(&form.csrf) {
        return edit_page(&ctx, &message, title, &form.message, Some("Votre session a expiré, veuillez réessayer.")).await;
    }
    if is_first && (title.is_empty() || text::url_title(title).is_empty() || !fits(title, 100)) {
        return edit_page(&ctx, &message, title, &form.message, Some("Le titre est obligatoire (100 caractères maximum).")).await;
    }
    let Some(body) = prepare(&form.message) else {
        return edit_page(&ctx, &message, title, &form.message, Some("Le message est vide ou trop long.")).await;
    };
    let db = &ctx.state.db;
    sqlx::query("UPDATE nf_forum_messages SET message = ? WHERE message_id = ?")
        .bind(body)
        .bind(message_id)
        .execute(db)
        .await?;
    let mut topic_title = message.topic.title.clone();
    if is_first {
        topic_title = text::encode(title);
        sqlx::query("UPDATE nf_forum_topics SET title = ? WHERE topic_id = ?")
            .bind(&topic_title)
            .bind(message.topic.topic_id)
            .execute(db)
            .await?;
    }
    Ok(Redirect::to(&format!(
        "{}#message-{message_id}",
        ctx.url(&topic_path(message.topic.topic_id, &topic_title))
    ))
    .into_response())
}

pub async fn delete_message(Extension(ctx): Ctxt, Path(message_id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    if !ctx.enabled("forum") {
        return layout::not_found(&ctx).await;
    }
    let Some(message) = find_message(&ctx, message_id).await? else {
        return layout::not_found(&ctx).await;
    };
    let topic = &message.topic;
    if !can(&ctx, "category_delete", topic.forum.category_id) {
        return layout::forbidden(&ctx).await;
    }
    // Le premier message porte le sujet : il se supprime avec lui.
    if ctx.csrf_ok(&form.csrf) && Some(message_id) != topic.first_message_id {
        // Les compteurs pointent peut-être sur ce message : on les détache d'abord.
        sqlx::query("UPDATE nf_forum_topics SET last_message_id = NULL WHERE last_message_id = ?")
            .bind(message_id)
            .execute(&ctx.state.db)
            .await?;
        sqlx::query("DELETE FROM nf_forum_messages WHERE message_id = ?")
            .bind(message_id)
            .execute(&ctx.state.db)
            .await?;
        recount(&ctx, Some(topic.topic_id), topic.forum_id).await?;
    }
    Ok(Redirect::to(&ctx.url(&topic.path())).into_response())
}

#[derive(Deserialize)]
pub struct ModerationForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    forum_id: u32,
}

/// Actions de modération sur un sujet : `announce`, `lock`, `delete`, `move`.
pub async fn moderate(Extension(ctx): Ctxt, Path((action, topic_id)): Path<(String, u32)>, Form(form): Form<ModerationForm>) -> Reply {
    if !ctx.enabled("forum") {
        return layout::not_found(&ctx).await;
    }
    let Some(topic) = find_topic(&ctx, topic_id).await? else {
        return layout::not_found(&ctx).await;
    };
    let category = topic.forum.category_id;
    let permission = match action.as_str() {
        "announce" => "category_announce",
        "lock" => "category_lock",
        "delete" => "category_delete",
        "move" => "category_move",
        _ => return layout::not_found(&ctx).await,
    };
    if !can(&ctx, "category_read", category) || !can(&ctx, permission, category) {
        return layout::forbidden(&ctx).await;
    }
    let back = Redirect::to(&ctx.url(&topic.path())).into_response();
    if !ctx.csrf_ok(&form.csrf) {
        return Ok(back);
    }
    let db = &ctx.state.db;
    match action.as_str() {
        "announce" | "lock" => {
            let (announce, locked) = if action == "announce" {
                (!topic.announce, topic.locked)
            } else {
                (topic.announce, !topic.locked)
            };
            sqlx::query("UPDATE nf_forum_topics SET status = ? WHERE topic_id = ?")
                .bind(Topic::status(announce, locked))
                .bind(topic_id)
                .execute(db)
                .await?;
            Ok(back)
        }
        "delete" => {
            // Le sujet et ses messages se référencent mutuellement : on détache puis on supprime.
            sqlx::query("UPDATE nf_forum SET last_message_id = NULL WHERE forum_id = ?").bind(topic.forum_id).execute(db).await?;
            sqlx::query("UPDATE nf_forum_topics SET message_id = NULL, last_message_id = NULL WHERE topic_id = ?").bind(topic_id).execute(db).await?;
            sqlx::query("DELETE FROM nf_forum_topics WHERE topic_id = ?").bind(topic_id).execute(db).await?;
            recount(&ctx, None, topic.forum_id).await?;
            Ok(Redirect::to(&ctx.url(&topic.forum.path())).into_response())
        }
        _ => {
            let Some(target) = find_forum(&ctx, form.forum_id).await? else {
                return layout::not_found(&ctx).await;
            };
            if target.url.is_some() || !can(&ctx, "category_read", target.category_id) {
                return layout::forbidden(&ctx).await;
            }
            sqlx::query("UPDATE nf_forum SET last_message_id = NULL WHERE forum_id = ?").bind(topic.forum_id).execute(db).await?;
            sqlx::query("UPDATE nf_forum_topics SET forum_id = ? WHERE topic_id = ?")
                .bind(target.forum_id)
                .bind(topic_id)
                .execute(db)
                .await?;
            recount(&ctx, None, topic.forum_id).await?;
            recount(&ctx, None, target.forum_id).await?;
            Ok(back)
        }
    }
}

pub async fn mark_all_read(Extension(ctx): Ctxt, Form(form): Form<CsrfOnly>) -> Reply {
    if let (Some(user), true) = (&ctx.user, ctx.csrf_ok(&form.csrf)) {
        let db = &ctx.state.db;
        sqlx::query(
            "INSERT INTO nf_forum_read (user_id, forum_id, date) SELECT ?, forum_id, NOW() FROM nf_forum \
             ON DUPLICATE KEY UPDATE date = NOW()",
        )
        .bind(user.id)
        .execute(db)
        .await?;
        sqlx::query("DELETE FROM nf_forum_topics_read WHERE user_id = ?").bind(user.id).execute(db).await?;
    }
    Ok(Redirect::to(&ctx.url("forum")).into_response())
}

pub async fn index(Extension(ctx): Ctxt) -> Reply {
    dispatch(&ctx, "").await
}

/// Toutes les pages `forum/...` en lecture.
pub async fn route(Extension(ctx): Ctxt, Path(rest): Path<String>) -> Reply {
    dispatch(&ctx, &rest).await
}

async fn dispatch(ctx: &Ctx, rest: &str) -> Reply {
    if !ctx.enabled("forum") {
        return layout::not_found(ctx).await;
    }
    let mut segments: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    let mut page_no = 1;
    if let [.., "page", n] = segments.as_slice() {
        match n.parse::<u32>() {
            Ok(n) if n >= 1 => page_no = n,
            _ => return layout::not_found(ctx).await,
        }
        segments.truncate(segments.len() - 2);
    }
    let id = |raw: &str| raw.parse::<u32>().ok();
    match segments.as_slice() {
        [] => index_page(ctx).await,
        ["topic", raw, slug] => match id(raw) {
            Some(topic_id) => topic_page(ctx, topic_id, slug, page_no).await,
            None => layout::not_found(ctx).await,
        },
        ["new", raw, _slug] => match id(raw) {
            Some(forum_id) => match writable_forum(ctx, forum_id).await? {
                Ok(forum) => new_topic_page(ctx, &forum, "", "", None).await,
                Err(reply) => reply,
            },
            None => layout::not_found(ctx).await,
        },
        ["message", "edit", raw, _slug] => {
            let message = match id(raw) {
                Some(message_id) => find_message(ctx, message_id).await?,
                None => None,
            };
            match message {
                Some(message) if can_edit(ctx, &message) => {
                    let title = text::decode(&message.topic.title);
                    let body = text::decode(&message.text);
                    edit_page(ctx, &message, &title, &body, None).await
                }
                Some(_) => layout::forbidden(ctx).await,
                None => layout::not_found(ctx).await,
            }
        }
        [raw, slug] => match id(raw) {
            Some(forum_id) => forum_page(ctx, forum_id, slug, page_no).await,
            None => layout::not_found(ctx).await,
        },
        _ => layout::not_found(ctx).await,
    }
}

// ---------------------------------------------------------------------------
// Widgets

/// Widgets du forum : `topics` (derniers sujets), `activity` / `index` (derniers
/// messages) et `statistics`.
pub async fn widget(ctx: &Ctx, kind: &str) -> Result<Vec<Block>, AppError> {
    let db = &ctx.state.db;
    let readable = readable_categories(ctx).await?;
    if kind == "statistics" {
        let count = |sql: &'static str| sqlx::query_scalar::<_, i64>(sql).fetch_one(db);
        let topics = count("SELECT COUNT(*) FROM nf_forum_topics").await?;
        let messages = count("SELECT COUNT(*) FROM nf_forum_messages").await?;
        let authors = count("SELECT COUNT(DISTINCT user_id) FROM nf_forum_messages").await?;
        let line = |label: &str, value: i64| {
            format!("<li class=\"list-group-item\"><span class=\"badge badge-secondary float-right\">{value}</span>{label}</li>")
        };
        return Ok(vec![Block::Panel(Panel::new().heading("Statistiques du forum", "fas fa-signal").body_raw(format!(
            "<ul class=\"list-group list-group-flush\">{}{}{}</ul>",
            line("Sujets", topics),
            line("Messages", messages),
            line("Participants", authors)
        )))]);
    }

    let by_topic = kind == "topics";
    let sql = format!(
        "SELECT t.topic_id, t.title, m.message_id, CAST(m.date AS DATETIME) AS date, u.id AS user_id, u.username \
         FROM nf_forum_messages m \
         INNER JOIN nf_forum_topics t ON t.topic_id = m.topic_id \
         INNER JOIN nf_forum f ON f.forum_id = t.forum_id {PARENT_JOIN} \
         LEFT JOIN nf_user u ON u.id = m.user_id AND u.deleted = '0' \
         WHERE {CATEGORY_OF} IN ({readable}){} ORDER BY m.message_id DESC LIMIT 5",
        if by_topic { " AND m.message_id = t.message_id" } else { "" }
    );
    let rows = sqlx::query(&sql).fetch_all(db).await?;
    let (title, empty) = if by_topic {
        ("Derniers sujets", "Aucun sujet pour le moment")
    } else {
        ("Activité du forum", "Aucun message pour le moment")
    };
    let panel = Panel::new().heading(title, ICON);
    if rows.is_empty() {
        return Ok(vec![Block::Panel(panel.body(empty))]);
    }
    let mut items = String::new();
    for row in &rows {
        let topic_id: u32 = row.try_get("topic_id")?;
        let topic_title = text::clean(&row.try_get::<String, _>("title")?);
        let date: NaiveDateTime = row.try_get("date")?;
        let author = match (row.try_get::<Option<u32>, _>("user_id")?, row.try_get::<Option<String>, _>("username")?) {
            (Some(id), Some(name)) => user::link(ctx.base(), id, &name),
            _ => "Visiteur".to_owned(),
        };
        items.push_str(&format!(
            "<li class=\"list-group-item\"><a href=\"{}\">{}</a><br /><small class=\"text-muted\">{author} — {}</small></li>",
            ctx.url(&topic_path(topic_id, &topic_title)),
            text::shorten(&topic_title, 45),
            text::date_time(&date)
        ));
    }
    let footer = format!("<a href=\"{}\">{} Accéder au forum</a>", ctx.url("forum"), text::icon("far fa-arrow-alt-circle-right"));
    Ok(vec![Block::Panel(
        panel
            .body_raw(format!("<ul class=\"list-group list-group-flush\">{items}</ul>"))
            .footer(footer, true),
    )])
}

// ---------------------------------------------------------------------------
// Administration

pub async fn admin_list(Extension(ctx): Ctxt, Query(notice): Query<Notice>) -> Reply {
    admin_guard!(ctx, "forum");
    let db = &ctx.state.db;
    let categories = sqlx::query("SELECT category_id, title FROM nf_forum_categories ORDER BY `order`, category_id")
        .fetch_all(db)
        .await?;
    let mut content = format!(
        "<div class=\"card mb-4\"><div class=\"card-body\"><h5>Nouvelle catégorie</h5><form method=\"post\" action=\"{}\" class=\"form-inline\">{}\
         <input type=\"text\" class=\"form-control mr-2\" name=\"title\" maxlength=\"100\" placeholder=\"Titre\" required />\
         <button type=\"submit\" class=\"btn btn-primary\">Ajouter</button></form></div></div>{}",
        ctx.url("admin/forum/categories/add"),
        form::csrf_field(&ctx),
        form::alert("success", notice.message()),
    );
    for category in &categories {
        let category_id: u32 = category.try_get("category_id")?;
        let forums = sqlx::query(
            "SELECT f.forum_id, f.title, f.description, (f.is_subforum = '1') AS sub, CAST(f.count_topics AS SIGNED) AS count_topics, u.url \
             FROM nf_forum f LEFT JOIN nf_forum pf ON f.is_subforum = '1' AND pf.forum_id = f.parent_id \
             LEFT JOIN nf_forum_url u ON u.forum_id = f.forum_id \
             WHERE IF(f.is_subforum = '1', pf.parent_id, f.parent_id) = ? \
             ORDER BY IF(f.is_subforum = '1', pf.`order`, f.`order`), IF(f.is_subforum = '1', pf.forum_id, f.forum_id), f.is_subforum, f.`order`, f.forum_id",
        )
        .bind(category_id)
        .fetch_all(db)
        .await?;
        let mut rows = String::new();
        for row in &forums {
            let forum_id: u32 = row.try_get("forum_id")?;
            let title = text::clean(&row.try_get::<String, _>("title")?);
            rows.push_str(&format!(
                "<tr><td>{}{title}{}</td><td>{}</td><td>{}</td>{}</tr>",
                if row.try_get::<i64, _>("sub")? != 0 { "<span class=\"text-muted ml-3\">↳</span> " } else { "" },
                if row.try_get::<Option<String>, _>("url")?.is_some() { " <span class=\"badge badge-info\">Lien</span>" } else { "" },
                text::clean(&row.try_get::<String, _>("description")?),
                row.try_get::<i64, _>("count_topics")?,
                admin::row_actions(
                    &ctx,
                    &format!("admin/forum/{forum_id}/{}", text::url_title(&title)),
                    &format!("admin/forum/delete/{forum_id}"),
                    "Supprimer ce forum, ses sous-forums et tous leurs sujets ?"
                ),
            ));
        }
        content.push_str(&format!(
            "<div class=\"card mb-4\"><div class=\"card-body\"><div class=\"float-right\">{} <a class=\"btn btn-primary btn-sm\" href=\"{}\">{} Ajouter un forum</a></div><h5>{}</h5></div>\
             <table class=\"table table-hover mb-0\"><thead><tr><th>Forum</th><th>Description</th><th>Sujets</th><th></th></tr></thead><tbody>{rows}</tbody></table></div>",
            admin::delete_button(&ctx, &format!("admin/forum/categories/delete/{category_id}"), "Supprimer cette catégorie et tous ses forums ?"),
            ctx.url(&format!("admin/forum/add?category={category_id}")),
            text::icon("fas fa-plus"),
            text::clean(&category.try_get::<String, _>("title")?),
        ));
    }
    admin::render(&ctx, "forum", TITLE, None, content)
}

#[derive(Deserialize)]
pub struct TitleForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    title: String,
}

pub async fn admin_category_add(Extension(ctx): Ctxt, Form(form): Form<TitleForm>) -> Reply {
    admin_guard!(ctx, "forum");
    admin::check_csrf(&ctx, &form.csrf)?;
    let title = form.title.trim();
    if title.is_empty() || !fits(title, 100) {
        return Err(AppError::BadRequest("Le titre est obligatoire (100 caractères maximum)."));
    }
    let mut tx = ctx.state.db.begin().await?;
    let category_id = sqlx::query("INSERT INTO nf_forum_categories (title, `order`) VALUES (?, 0)")
        .bind(text::encode(title))
        .execute(&mut *tx)
        .await?
        .last_insert_id();
    for (action, detail) in DEFAULT_ACCESS {
        let access_id = sqlx::query("INSERT INTO nf_access (id, module, action) VALUES (?, 'forum', ?)")
            .bind(category_id)
            .bind(action)
            .execute(&mut *tx)
            .await?
            .last_insert_id();
        if let Some((entity, authorized)) = detail {
            sqlx::query("INSERT INTO nf_access_details (access_id, entity, type, authorized) VALUES (?, ?, 'group', ?)")
                .bind(access_id)
                .bind(entity)
                .bind(if authorized { "1" } else { "0" })
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    admin::redirect(&ctx, "admin/forum?ok=1")
}

/// Supprime des forums (liste SQL d'identifiants) avec leurs sujets et messages.
async fn delete_forums(ctx: &Ctx, ids: &[u32]) -> Result<(), AppError> {
    let db = &ctx.state.db;
    for forum_id in ids {
        sqlx::query("UPDATE nf_forum SET last_message_id = NULL WHERE forum_id = ?").bind(forum_id).execute(db).await?;
        sqlx::query("UPDATE nf_forum_topics SET message_id = NULL, last_message_id = NULL WHERE forum_id = ?").bind(forum_id).execute(db).await?;
        sqlx::query("DELETE FROM nf_forum_read WHERE forum_id = ?").bind(forum_id).execute(db).await?;
        // Les sujets (et par cascade leurs messages) partent avec le forum.
        sqlx::query("DELETE FROM nf_forum WHERE forum_id = ?").bind(forum_id).execute(db).await?;
    }
    Ok(())
}

pub async fn admin_category_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx, "forum");
    admin::check_csrf(&ctx, &form.csrf)?;
    let db = &ctx.state.db;
    let forums: Vec<u32> = sqlx::query_scalar(
        "SELECT f.forum_id FROM nf_forum f LEFT JOIN nf_forum pf ON f.is_subforum = '1' AND pf.forum_id = f.parent_id \
         WHERE IF(f.is_subforum = '1', pf.parent_id, f.parent_id) = ? ORDER BY f.is_subforum DESC",
    )
    .bind(id)
    .fetch_all(db)
    .await?;
    delete_forums(&ctx, &forums).await?;
    sqlx::query("DELETE FROM nf_access WHERE module = 'forum' AND id = ?").bind(id).execute(db).await?;
    sqlx::query("DELETE FROM nf_forum_categories WHERE category_id = ?").bind(id).execute(db).await?;
    admin::redirect(&ctx, "admin/forum?ok=1")
}

#[derive(Deserialize, Default)]
pub struct ForumForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    /// `c<id>` : dans une catégorie ; `f<id>` : sous-forum d'un forum.
    #[serde(default)]
    parent: String,
    #[serde(default)]
    url: String,
}

#[derive(Deserialize)]
pub struct AddQuery {
    category: Option<u32>,
}

async fn forum_form(ctx: &Ctx, action: &str, form: &ForumForm, editing: bool, error: Option<&str>) -> Reply {
    let db = &ctx.state.db;
    let mut options: Vec<(String, String)> = Vec::new();
    for row in sqlx::query("SELECT category_id, title FROM nf_forum_categories ORDER BY `order`, category_id").fetch_all(db).await? {
        let category_id: u32 = row.try_get("category_id")?;
        options.push((format!("c{category_id}"), text::decode(&row.try_get::<String, _>("title")?)));
        let forums = sqlx::query(
            "SELECT f.forum_id, f.title FROM nf_forum f LEFT JOIN nf_forum_url u ON u.forum_id = f.forum_id \
             WHERE f.is_subforum = '0' AND f.parent_id = ? AND u.forum_id IS NULL ORDER BY f.`order`, f.forum_id",
        )
        .bind(category_id)
        .fetch_all(db)
        .await?;
        for forum in forums {
            options.push((
                format!("f{}", forum.try_get::<u32, _>("forum_id")?),
                format!("— sous-forum de : {}", text::decode(&forum.try_get::<String, _>("title")?)),
            ));
        }
    }
    let fields = format!(
        "{}{}{}{}",
        input("Titre", "title", &form.title, 100, true, ""),
        input("Description", "description", &form.description, 100, false, ""),
        if editing {
            String::new()
        } else {
            form::select("Emplacement", "parent", &form.parent, options.iter().map(|(v, t)| (v.as_str(), t.as_str())))
        },
        input("Lien", "url", &form.url, 100, false, "Facultatif : transforme le forum en lien vers cette adresse (https://…)."),
    );
    let title = if editing { "Modifier le forum" } else { "Ajouter un forum" };
    admin::render(ctx, "forum", title, None, admin::form_card(ctx, action, error, fields, "admin/forum"))
}

fn validate_forum(form: &ForumForm) -> Result<(), &'static str> {
    let title = form.title.trim();
    if title.is_empty() || text::url_title(title).is_empty() || !fits(title, 100) {
        return Err("Le titre est obligatoire (100 caractères maximum).");
    }
    if !fits(form.description.trim(), 100) {
        return Err("La description est limitée à 100 caractères.");
    }
    let url = form.url.trim();
    let url_ok = url.is_empty()
        || ((url.starts_with("https://") || url.starts_with("http://"))
            && fits(url, 100)
            && !url.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '<' | '>')));
    if !url_ok {
        return Err("Le lien doit être une adresse complète (https://…).");
    }
    Ok(())
}

async fn set_url(ctx: &Ctx, forum_id: u32, url: &str) -> Result<(), AppError> {
    let db = &ctx.state.db;
    if url.is_empty() {
        sqlx::query("DELETE FROM nf_forum_url WHERE forum_id = ?").bind(forum_id).execute(db).await?;
    } else {
        sqlx::query("INSERT INTO nf_forum_url (forum_id, url, redirects) VALUES (?, ?, 0) ON DUPLICATE KEY UPDATE url = VALUES(url)")
            .bind(forum_id)
            .bind(text::encode(url))
            .execute(db)
            .await?;
    }
    Ok(())
}

pub async fn admin_add_get(Extension(ctx): Ctxt, Query(query): Query<AddQuery>) -> Reply {
    admin_guard!(ctx, "forum");
    let form = ForumForm {
        parent: query.category.map(|c| format!("c{c}")).unwrap_or_default(),
        ..ForumForm::default()
    };
    forum_form(&ctx, "admin/forum/add", &form, false, None).await
}

pub async fn admin_add_post(Extension(ctx): Ctxt, Form(form): Form<ForumForm>) -> Reply {
    admin_guard!(ctx, "forum");
    admin::check_csrf(&ctx, &form.csrf)?;
    if let Err(message) = validate_forum(&form) {
        return forum_form(&ctx, "admin/forum/add", &form, false, Some(message)).await;
    }
    let db = &ctx.state.db;
    let parent_id = form.parent.get(1..).and_then(|raw| raw.parse::<u32>().ok());
    let parent = match (form.parent.chars().next(), parent_id) {
        (Some('c'), Some(id)) => sqlx::query("SELECT 1 FROM nf_forum_categories WHERE category_id = ?")
            .bind(id)
            .fetch_optional(db)
            .await?
            .map(|_| (id, "0")),
        (Some('f'), Some(id)) => sqlx::query("SELECT 1 FROM nf_forum WHERE forum_id = ? AND is_subforum = '0'")
            .bind(id)
            .fetch_optional(db)
            .await?
            .map(|_| (id, "1")),
        _ => None,
    };
    let Some((parent_id, is_subforum)) = parent else {
        return forum_form(&ctx, "admin/forum/add", &form, false, Some("Emplacement invalide.")).await;
    };
    let forum_id = sqlx::query(
        "INSERT INTO nf_forum (parent_id, is_subforum, title, description, `order`, count_topics, count_messages) VALUES (?, ?, ?, ?, 0, 0, 0)",
    )
    .bind(parent_id)
    .bind(is_subforum)
    .bind(text::encode(form.title.trim()))
    .bind(text::encode(form.description.trim()))
    .execute(db)
    .await?
    .last_insert_id() as u32;
    set_url(&ctx, forum_id, form.url.trim()).await?;
    admin::redirect(&ctx, "admin/forum?ok=1")
}

pub async fn admin_edit_get(Extension(ctx): Ctxt, Path((id, _slug)): Path<(u32, String)>) -> Reply {
    admin_guard!(ctx, "forum");
    let Some(forum) = find_forum(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let form = ForumForm {
        csrf: String::new(),
        title: text::decode(&forum.title),
        description: text::decode(&forum.description),
        parent: String::new(),
        url: forum.url.as_deref().map(text::decode).unwrap_or_default(),
    };
    forum_form(&ctx, &format!("admin/forum/{id}/x"), &form, true, None).await
}

pub async fn admin_edit_post(Extension(ctx): Ctxt, Path((id, _slug)): Path<(u32, String)>, Form(form): Form<ForumForm>) -> Reply {
    admin_guard!(ctx, "forum");
    admin::check_csrf(&ctx, &form.csrf)?;
    if find_forum(&ctx, id).await?.is_none() {
        return layout::not_found(&ctx).await;
    }
    if let Err(message) = validate_forum(&form) {
        return forum_form(&ctx, &format!("admin/forum/{id}/x"), &form, true, Some(message)).await;
    }
    sqlx::query("UPDATE nf_forum SET title = ?, description = ? WHERE forum_id = ?")
        .bind(text::encode(form.title.trim()))
        .bind(text::encode(form.description.trim()))
        .bind(id)
        .execute(&ctx.state.db)
        .await?;
    set_url(&ctx, id, form.url.trim()).await?;
    admin::redirect(&ctx, "admin/forum?ok=1")
}

pub async fn admin_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx, "forum");
    admin::check_csrf(&ctx, &form.csrf)?;
    let mut ids: Vec<u32> = sqlx::query_scalar("SELECT forum_id FROM nf_forum WHERE is_subforum = '1' AND parent_id = ?")
        .bind(id)
        .fetch_all(&ctx.state.db)
        .await?;
    ids.push(id);
    delete_forums(&ctx, &ids).await?;
    admin::redirect(&ctx, "admin/forum?ok=1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_status_codes() {
        assert_eq!(Topic::status(false, false), "0");
        assert_eq!(Topic::status(true, false), "1");
        assert_eq!(Topic::status(false, true), "-1");
        assert_eq!(Topic::status(true, true), "-2");
    }

    #[test]
    fn messages() {
        assert_eq!(prepare(" <b>x</b>\r\ny "), Some("&lt;b&gt;x&lt;/b&gt;\ny".to_owned()));
        assert_eq!(prepare("  "), None);
        assert_eq!(prepare(&"x".repeat(MAX_MESSAGE + 1)), None);
    }

    #[test]
    fn forum_validation() {
        let ok = ForumForm { title: "Général".into(), ..ForumForm::default() };
        assert!(validate_forum(&ok).is_ok());
        assert!(validate_forum(&ForumForm::default()).is_err());
        assert!(validate_forum(&ForumForm { url: "javascript:alert(1)".into(), ..ForumForm { title: "x".into(), ..ForumForm::default() } }).is_err());
        assert!(validate_forum(&ForumForm { url: "https://discord.gg/abc".into(), ..ForumForm { title: "x".into(), ..ForumForm::default() } }).is_ok());
    }
}
