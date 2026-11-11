//! Messagerie privée entre membres (partie « messages » de `modules/user`).
//!
//! Une conversation (`nf_users_messages`) a des participants
//! (`nf_users_messages_recipients`, auteur compris) et des réponses
//! (`nf_users_messages_replies`). `recipients.date` est la date de dernière lecture ;
//! `deleted` masque la conversation pour ce participant jusqu'à la prochaine réponse.

use super::admin::{self, Ctxt, CsrfOnly, Reply};
use crate::{
    core::{
        bbcode,
        ctx::Ctx,
        form::{self, fits, input},
        layout::{self, Page},
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

const TITLE: &str = "Messagerie";
const ICON: &str = "far fa-envelope";
const MAX_RECIPIENTS: usize = 10;
const MAX_LENGTH: usize = 10_000;

fn login_redirect(ctx: &Ctx) -> Reply {
    Ok(Redirect::to(&ctx.url("user/login")).into_response())
}

fn page(title: impl Into<String>) -> Page {
    Page::new("user", "Espace membre", "fas fa-user").title(title).crumb(TITLE, "user/messages")
}

/// Nombre de conversations avec des réponses non lues.
pub async fn unread(ctx: &Ctx) -> Result<i64, AppError> {
    let Some(me) = &ctx.user else {
        return Ok(0);
    };
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM nf_users_messages_recipients r \
         INNER JOIN nf_users_messages m ON m.message_id = r.message_id \
         INNER JOIN nf_users_messages_replies l ON l.reply_id = m.last_reply_id \
         WHERE r.user_id = ? AND r.deleted = '0' AND l.user_id <> r.user_id AND (r.date IS NULL OR r.date < l.date)",
    )
    .bind(me.id)
    .fetch_one(&ctx.state.db)
    .await?)
}

pub async fn inbox(Extension(ctx): Ctxt) -> Reply {
    let Some(me) = &ctx.user else {
        return login_redirect(&ctx);
    };
    let rows = sqlx::query(
        "SELECT m.message_id, m.title, CAST(l.date AS DATETIME) AS last_date, l.user_id AS last_user_id, u.username AS last_username, \
                (l.user_id <> r.user_id AND (r.date IS NULL OR r.date < l.date)) AS unread, \
                (SELECT COUNT(*) FROM nf_users_messages_replies x WHERE x.message_id = m.message_id) AS replies \
         FROM nf_users_messages_recipients r \
         INNER JOIN nf_users_messages m ON m.message_id = r.message_id \
         INNER JOIN nf_users_messages_replies l ON l.reply_id = m.last_reply_id \
         INNER JOIN nf_user u ON u.id = l.user_id \
         WHERE r.user_id = ? AND r.deleted = '0' ORDER BY l.reply_id DESC LIMIT 100",
    )
    .bind(me.id)
    .fetch_all(&ctx.state.db)
    .await?;
    let mut items = String::new();
    for row in &rows {
        let id: u32 = row.try_get("message_id")?;
        let title = text::clean(&row.try_get::<String, _>("title")?);
        let unread = row.try_get::<i64, _>("unread")? != 0;
        let last: NaiveDateTime = row.try_get("last_date")?;
        items.push_str(&format!(
            "<tr><td>{}<a href=\"{}\">{}</a></td><td>{}</td><td>{}<br /><small class=\"text-muted\">{}</small></td><td class=\"text-right\">\
             <form method=\"post\" action=\"{}\" class=\"d-inline\">{}<button type=\"submit\" class=\"btn btn-link btn-sm text-danger p-0\" title=\"Supprimer\">{}</button></form></td></tr>",
            if unread { "<span class=\"badge badge-danger mr-1\">Nouveau</span>" } else { "" },
            ctx.url(&format!("user/messages/{id}/{}", text::url_title(&title))),
            if unread { format!("<b>{title}</b>") } else { title.clone() },
            row.try_get::<i64, _>("replies")?,
            user::link(ctx.base(), row.try_get("last_user_id")?, &text::clean(&row.try_get::<String, _>("last_username")?)),
            text::date_time(&last),
            ctx.url(&format!("user/messages/delete/{id}")),
            form::csrf_field(&ctx),
            text::icon("far fa-trash-alt"),
        ));
    }
    let body = if items.is_empty() {
        "<div class=\"card-body text-center\">Aucun message</div>".to_owned()
    } else {
        format!(
            "<table class=\"table table-hover mb-0\"><thead><tr><th>Sujet</th><th>Messages</th><th>Dernier message</th><th></th></tr></thead><tbody>{items}</tbody></table>"
        )
    };
    let footer = format!(
        "<a class=\"btn btn-primary\" href=\"{}\">{} Nouveau message</a>",
        ctx.url("user/messages/compose"),
        text::icon("fas fa-pencil-alt")
    );
    let panel = Panel::new().heading(TITLE, ICON).body_raw(body).footer(footer, true);
    layout::render(&ctx, page(TITLE).blocks(&[Block::Panel(panel)])).await
}

#[derive(Deserialize, Default)]
pub struct ComposeForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    recipients: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
pub struct ComposeQuery {
    #[serde(default)]
    to: String,
}

fn message_area(value: &str) -> String {
    format!(
        "<div class=\"form-group\"><label for=\"f-message\"><b>Message</b></label>\
         <textarea class=\"form-control\" id=\"f-message\" name=\"message\" rows=\"8\" maxlength=\"{MAX_LENGTH}\" required>{}</textarea>\
         <small class=\"form-text text-muted\">BBCode accepté.</small></div>",
        text::encode(value)
    )
}

async fn compose_page(ctx: &Ctx, values: &ComposeForm, error: Option<&str>) -> Reply {
    let body = format!(
        "{}<form method=\"post\" action=\"{}\">{}{}{}{}<button type=\"submit\" class=\"btn btn-primary\">Envoyer</button> \
         <a class=\"btn btn-link\" href=\"{}\">Annuler</a></form>",
        form::alert("danger", error),
        ctx.url("user/messages/compose"),
        form::csrf_field(ctx),
        input("Destinataires", "recipients", &values.recipients, 500, true, "Noms d'utilisateur séparés par des virgules (10 au maximum)."),
        input("Sujet", "title", &values.title, 100, true, ""),
        message_area(&values.message),
        ctx.url("user/messages"),
    );
    let panel = Panel::new().heading("Nouveau message", "fas fa-pencil-alt").body(body);
    layout::render(ctx, page("Nouveau message").crumb("Nouveau message", "user/messages/compose").blocks(&[Block::Panel(panel)])).await
}

pub async fn compose_get(Extension(ctx): Ctxt, Query(query): Query<ComposeQuery>) -> Reply {
    if ctx.user.is_none() {
        return login_redirect(&ctx);
    }
    let values = ComposeForm { recipients: query.to.chars().take(100).collect(), ..ComposeForm::default() };
    compose_page(&ctx, &values, None).await
}

/// Découpe la liste saisie : noms distincts, sans vide.
fn recipient_names(raw: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in raw.split([',', ';']).map(str::trim).filter(|n| !n.is_empty()) {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            names.push(name.to_owned());
        }
    }
    names
}

fn throttled(ctx: &Ctx, user_id: u32) -> bool {
    let key = format!("messages:{user_id}");
    let hour = Duration::from_secs(3600);
    if throttle::blocked(&ctx.state, &key, 30, hour) {
        return true;
    }
    throttle::hit(&ctx.state, &key, hour);
    false
}

pub async fn compose_post(Extension(ctx): Ctxt, Form(mut form): Form<ComposeForm>) -> Reply {
    let Some(me) = &ctx.user else {
        return login_redirect(&ctx);
    };
    if !ctx.csrf_ok(&form.csrf) {
        return compose_page(&ctx, &form, Some("Votre session a expiré, veuillez réessayer.")).await;
    }
    form.title = form.title.trim().to_owned();
    form.message = form.message.trim().to_owned();
    let names = recipient_names(&form.recipients);
    let db = &ctx.state.db;

    let mut ids: Vec<u32> = Vec::new();
    let mut unknown = None;
    for name in names.iter().take(MAX_RECIPIENTS + 1) {
        let id: Option<u32> = sqlx::query_scalar("SELECT id FROM nf_user WHERE username = ? AND deleted = '0'")
            .bind(text::encode(name))
            .fetch_optional(db)
            .await?;
        match id {
            Some(id) if id != me.id && !ids.contains(&id) => ids.push(id),
            Some(_) => {}
            None => unknown = Some(format!("Destinataire inconnu : {}.", text::encode(name))),
        }
    }
    let error = if let Some(message) = unknown {
        Some(message)
    } else if ids.is_empty() {
        Some("Indiquez au moins un destinataire (autre que vous).".to_owned())
    } else if names.len() > MAX_RECIPIENTS {
        Some("Dix destinataires au maximum.".to_owned())
    } else if form.title.is_empty() || !fits(&form.title, 100) {
        Some("Le sujet est obligatoire (100 caractères maximum).".to_owned())
    } else if form.message.is_empty() || form.message.chars().count() > MAX_LENGTH {
        Some("Le message est obligatoire (10 000 caractères maximum).".to_owned())
    } else if throttled(&ctx, me.id) {
        Some("Trop de messages envoyés, réessayez plus tard.".to_owned())
    } else {
        None
    };
    if let Some(message) = error {
        return compose_page(&ctx, &form, Some(&message)).await;
    }

    let mut tx = db.begin().await?;
    let message_id = sqlx::query("INSERT INTO nf_users_messages (title) VALUES (?)")
        .bind(text::encode(&form.title))
        .execute(&mut *tx)
        .await?
        .last_insert_id();
    let reply_id = sqlx::query("INSERT INTO nf_users_messages_replies (message_id, user_id, message) VALUES (?, ?, ?)")
        .bind(message_id)
        .bind(me.id)
        .bind(text::encode(&form.message))
        .execute(&mut *tx)
        .await?
        .last_insert_id();
    sqlx::query("UPDATE nf_users_messages SET reply_id = ?, last_reply_id = ? WHERE message_id = ?")
        .bind(reply_id)
        .bind(reply_id)
        .bind(message_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO nf_users_messages_recipients (user_id, message_id, date) VALUES (?, ?, NOW())")
        .bind(me.id)
        .bind(message_id)
        .execute(&mut *tx)
        .await?;
    for id in &ids {
        sqlx::query("INSERT INTO nf_users_messages_recipients (user_id, message_id) VALUES (?, ?)")
            .bind(id)
            .bind(message_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Redirect::to(&ctx.url(&format!("user/messages/{message_id}/{}", text::url_title(&form.title)))).into_response())
}

/// La conversation, si l'utilisateur courant y participe.
async fn conversation(ctx: &Ctx, me: u32, id: u32) -> Result<Option<String>, AppError> {
    let title: Option<String> = sqlx::query_scalar(
        "SELECT m.title FROM nf_users_messages m INNER JOIN nf_users_messages_recipients r ON r.message_id = m.message_id \
         WHERE m.message_id = ? AND r.user_id = ?",
    )
    .bind(id)
    .bind(me)
    .fetch_optional(&ctx.state.db)
    .await?;
    Ok(title.map(|t| text::clean(&t)))
}

pub async fn show(Extension(ctx): Ctxt, Path((id, _slug)): Path<(u32, String)>) -> Reply {
    let Some(me) = &ctx.user else {
        return login_redirect(&ctx);
    };
    // Conversation d'autrui : même réponse qu'une conversation inexistante.
    let Some(title) = conversation(&ctx, me.id, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let db = &ctx.state.db;
    sqlx::query("UPDATE nf_users_messages_recipients SET date = NOW() WHERE message_id = ? AND user_id = ?")
        .bind(id)
        .bind(me.id)
        .execute(db)
        .await?;

    let participants = sqlx::query(
        "SELECT u.id, u.username FROM nf_users_messages_recipients r INNER JOIN nf_user u ON u.id = r.user_id WHERE r.message_id = ? ORDER BY u.username",
    )
    .bind(id)
    .fetch_all(db)
    .await?;
    let mut who = Vec::new();
    for row in &participants {
        who.push(user::link(ctx.base(), row.try_get("id")?, &text::clean(&row.try_get::<String, _>("username")?)));
    }
    let replies = sqlx::query(
        "SELECT l.user_id, u.username, l.message, CAST(l.date AS DATETIME) AS date FROM nf_users_messages_replies l \
         INNER JOIN nf_user u ON u.id = l.user_id WHERE l.message_id = ? ORDER BY l.reply_id",
    )
    .bind(id)
    .fetch_all(db)
    .await?;
    let mut blocks = vec![Block::Panel(
        Panel::new().heading(title.clone(), ICON).body(format!("Participants : {}", who.join(", "))),
    )];
    for row in &replies {
        let date: NaiveDateTime = row.try_get("date")?;
        blocks.push(Block::Panel(Panel::new().body(format!(
            "<p class=\"text-muted mb-2\"><b>{}</b> · <small>{}</small></p>{}",
            user::link(ctx.base(), row.try_get("user_id")?, &text::clean(&row.try_get::<String, _>("username")?)),
            text::date_time(&date),
            bbcode::render(&text::clean(&row.try_get::<String, _>("message")?)),
        ))));
    }
    blocks.push(Block::Panel(Panel::new().heading("Répondre", "fas fa-reply").body(format!(
        "<form method=\"post\" action=\"{}\">{}{}<button type=\"submit\" class=\"btn btn-primary\">Envoyer</button></form>",
        ctx.url(&format!("user/messages/{id}/reply")),
        form::csrf_field(&ctx),
        message_area(""),
    ))));
    let path = format!("user/messages/{id}/{}", text::url_title(&title));
    layout::render(&ctx, page(title.clone()).crumb(title, path).blocks(&blocks)).await
}

#[derive(Deserialize)]
pub struct ReplyForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    message: String,
}

pub async fn reply(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<ReplyForm>) -> Reply {
    let Some(me) = &ctx.user else {
        return login_redirect(&ctx);
    };
    admin::check_csrf(&ctx, &form.csrf)?;
    let Some(title) = conversation(&ctx, me.id, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let message = form.message.trim();
    if message.is_empty() || message.chars().count() > MAX_LENGTH {
        return Err(AppError::BadRequest("Le message est obligatoire (10 000 caractères maximum)."));
    }
    if throttled(&ctx, me.id) {
        return Err(AppError::BadRequest("Trop de messages envoyés, réessayez plus tard."));
    }
    let mut tx = ctx.state.db.begin().await?;
    let reply_id = sqlx::query("INSERT INTO nf_users_messages_replies (message_id, user_id, message) VALUES (?, ?, ?)")
        .bind(id)
        .bind(me.id)
        .bind(text::encode(message))
        .execute(&mut *tx)
        .await?
        .last_insert_id();
    sqlx::query("UPDATE nf_users_messages SET last_reply_id = ? WHERE message_id = ?")
        .bind(reply_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    // Une réponse fait réapparaître la conversation chez ceux qui l'avaient supprimée.
    sqlx::query("UPDATE nf_users_messages_recipients SET deleted = '0' WHERE message_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE nf_users_messages_recipients SET date = NOW() WHERE message_id = ? AND user_id = ?")
        .bind(id)
        .bind(me.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Redirect::to(&ctx.url(&format!("user/messages/{id}/{}", text::url_title(&title)))).into_response())
}

pub async fn delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    let Some(me) = &ctx.user else {
        return login_redirect(&ctx);
    };
    admin::check_csrf(&ctx, &form.csrf)?;
    sqlx::query("UPDATE nf_users_messages_recipients SET deleted = '1', date = NOW() WHERE message_id = ? AND user_id = ?")
        .bind(id)
        .bind(me.id)
        .execute(&ctx.state.db)
        .await?;
    Ok(Redirect::to(&ctx.url("user/messages")).into_response())
}

#[cfg(test)]
mod tests {
    use super::recipient_names;

    #[test]
    fn recipients_are_split_and_deduplicated() {
        assert_eq!(recipient_names(" bob, Alice ;bob,, BOB "), vec!["bob", "Alice"]);
        assert!(recipient_names(" , ").is_empty());
    }
}
