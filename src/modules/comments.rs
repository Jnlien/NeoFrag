//! Module Commentaires (`nf_comment`), porté de `modules/comments`.
//!
//! Un commentaire est rattaché à un contenu par le couple (module, identifiant). Seules
//! les actualités en accueillent pour l'instant ; chaque module porté s'ajoutera dans
//! [`target_path`]. Comme dans l'original, il faut être connecté pour commenter, et
//! l'auteur ou un administrateur peut supprimer. (Les réponses à un commentaire
//! étaient encore « TODO » dans l'original : l'affichage les gère, pas la saisie.)

use super::news;
use crate::{
    core::{
        ctx::Ctx,
        form, layout,
        panel::Panel,
        text, throttle, user,
    },
    error::AppError,
};
use axum::{
    extract::Path,
    response::{IntoResponse, Redirect, Response},
    Extension, Form,
};
use chrono::NaiveDateTime;
use serde::Deserialize;
use sqlx::Row;
use std::{sync::Arc, time::Duration};

pub const TITLE: &str = "Commentaires";
pub const ICON: &str = "far fa-comments";
const MAX_LENGTH: usize = 5000;

/// Chemin de la page qui affiche le contenu commenté, ou `None` s'il n'existe pas
/// (ou n'est pas visible) : on ne commente pas ce qu'on ne peut pas lire.
async fn target_path(ctx: &Ctx, module: &str, module_id: u32) -> Result<Option<String>, AppError> {
    Ok(match module {
        "news" => news::find(&ctx.state.db, ctx.lang(), module_id, true)
            .await?
            .map(|item| item.path()),
        "events" if ctx.enabled("events") => super::events::find_visible(ctx, module_id).await?.map(|event| event.path()),
        "gallery" if ctx.enabled("gallery") => super::gallery::find_visible_image_path(ctx, module_id).await?,
        _ => None,
    })
}

pub async fn count(ctx: &Ctx, module: &str, module_id: u32) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM nf_comment WHERE module = ? AND module_id = ?")
        .bind(module)
        .bind(module_id)
        .fetch_one(&ctx.state.db)
        .await?)
}

/// Panel des commentaires d'un contenu : formulaire d'ajout puis liste.
pub async fn panel(ctx: &Ctx, module: &'static str, module_id: u32) -> Result<String, AppError> {
    let rows = sqlx::query(
        "SELECT c.id, c.parent_id, c.user_id, c.content, CAST(c.date AS DATETIME) AS date, \
                u.username, IFNULL(u.deleted = '0', 0) AS alive, f.path AS avatar, up.sex \
         FROM nf_comment c \
         LEFT JOIN nf_user u ON u.id = c.user_id \
         LEFT JOIN nf_user_profile up ON up.id = u.id \
         LEFT JOIN nf_file f ON f.id = up.avatar \
         WHERE c.module = ? AND c.module_id = ? \
         ORDER BY IFNULL(c.parent_id, c.id) DESC, c.id ASC LIMIT 500",
    )
    .bind(module)
    .bind(module_id)
    .fetch_all(&ctx.state.db)
    .await?;

    let new = match &ctx.user {
        Some(_) => format!(
            "<form method=\"post\" action=\"{}\" class=\"mb-4\">{}\
             <input type=\"hidden\" name=\"module\" value=\"{module}\" /><input type=\"hidden\" name=\"module_id\" value=\"{module_id}\" />\
             <div class=\"form-group\"><textarea class=\"form-control\" name=\"comment\" rows=\"4\" maxlength=\"{MAX_LENGTH}\" placeholder=\"Votre commentaire...\" required></textarea></div>\
             <button type=\"submit\" class=\"btn btn-primary\">Envoyer</button></form>",
            ctx.url("comments/add"),
            form::csrf_field(ctx),
        ),
        None => format!(
            "<div class=\"alert alert-danger\" role=\"alert\">{} Vous devez être <a href=\"{}\">identifié</a> pour pouvoir poster un commentaire</div>",
            text::icon("fas fa-ban"),
            ctx.url("user/login"),
        ),
    };

    let mut list = String::new();
    for row in &rows {
        let id: u32 = row.try_get("id")?;
        let author_id: u32 = row.try_get("user_id")?;
        let is_child = row.try_get::<Option<u32>, _>("parent_id")?.is_some();
        let alive = row.try_get::<i64, _>("alive")? != 0;
        let username: Option<String> = row.try_get("username")?;
        let avatar: Option<String> = row.try_get("avatar")?;
        let sex: Option<String> = row.try_get("sex")?;
        let date: NaiveDateTime = row.try_get("date")?;
        let content = text::clean(&row.try_get::<String, _>("content")?);

        let can_delete = ctx.user.as_ref().is_some_and(|u| u.admin || u.id == author_id);
        let actions = if can_delete && !content.is_empty() {
            format!(
                "<ul class=\"list-inline float-right\"><li class=\"list-inline-item\">\
                 <form method=\"post\" action=\"{}\" onsubmit=\"return confirm('Êtes-vous sûr(e) de vouloir supprimer ce commentaire ?');\">{}\
                 <button type=\"submit\" class=\"btn btn-link btn-sm text-danger p-0\" title=\"Supprimer\">{}</button></form></li></ul>",
                ctx.url(&format!("comments/delete/{id}")),
                form::csrf_field(ctx),
                text::icon("far fa-trash-alt"),
            )
        } else {
            String::new()
        };
        let author = match username {
            Some(name) if alive => user::link(ctx.base(), author_id, &name),
            _ => "Visiteur".to_owned(),
        };
        let body = if content.is_empty() {
            "<i>Message supprimé</i>".to_owned()
        } else {
            text::linkify(&content).replace('\n', "<br />")
        };
        list.push_str(&format!(
            "<div class=\"media{}\" id=\"comment-{id}\">{}<div class=\"media-body\">{actions}<h6>{author} <small>{} {}</small></h6>{body}</div></div>",
            if is_child { " comments-child" } else { "" },
            user::avatar_html(ctx.base(), if alive { avatar.as_deref() } else { None }, sex.as_deref(), false, ""),
            text::icon("far fa-clock"),
            text::date_time(&date),
        ));
    }

    let heading = format!("{TITLE} <span class=\"badge badge-secondary float-right\">{}</span>", rows.len());
    Ok(Panel::new()
        .heading(heading, ICON)
        .style("comments")
        .body(format!("<a name=\"comments\"></a>{new}{list}"))
        .render())
}

#[derive(Deserialize)]
pub struct AddForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    module: String,
    #[serde(default)]
    module_id: u32,
    #[serde(default)]
    comment: String,
}

/// Texte d'un commentaire prêt à être enregistré, ou `None` s'il est vide ou trop long.
fn prepare(raw: &str) -> Option<String> {
    let comment = raw.replace("\r\n", "\n");
    let comment = comment.trim();
    (!comment.is_empty() && comment.chars().count() <= MAX_LENGTH).then(|| text::encode(comment))
}

pub async fn add(Extension(ctx): Extension<Arc<Ctx>>, Form(form): Form<AddForm>) -> Result<Response, AppError> {
    let Some(author) = &ctx.user else {
        return layout::forbidden(&ctx).await;
    };
    let Some(path) = target_path(&ctx, &form.module, form.module_id).await? else {
        return layout::not_found(&ctx).await;
    };
    let back = Redirect::to(&format!("{}#comments", ctx.url(&path))).into_response();
    if !ctx.csrf_ok(&form.csrf) {
        return Ok(back);
    }
    let Some(content) = prepare(&form.comment) else {
        return Ok(back);
    };

    let key = format!("comment:{}", author.id);
    let window = Duration::from_secs(5 * 60);
    if throttle::blocked(&ctx.state, &key, 10, window) {
        return Err(AppError::BadRequest("Trop de commentaires en peu de temps, patientez quelques minutes."));
    }
    throttle::hit(&ctx.state, &key, window);

    sqlx::query("INSERT INTO nf_comment (user_id, module_id, module, content) VALUES (?, ?, ?, ?)")
        .bind(author.id)
        .bind(form.module_id)
        .bind(&form.module)
        .bind(content)
        .execute(&ctx.state.db)
        .await?;
    Ok(back)
}

#[derive(Deserialize)]
pub struct DeleteForm {
    #[serde(default)]
    csrf: String,
}

pub async fn delete(
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<u32>,
    Form(form): Form<DeleteForm>,
) -> Result<Response, AppError> {
    let Some(current) = &ctx.user else {
        return layout::forbidden(&ctx).await;
    };
    let db = &ctx.state.db;
    let row = sqlx::query("SELECT user_id, module, module_id FROM nf_comment WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?;
    let Some(row) = row else {
        return layout::not_found(&ctx).await;
    };
    let author_id: u32 = row.try_get("user_id")?;
    let module: String = row.try_get("module")?;
    let module_id: u32 = row.try_get("module_id")?;
    if !(current.admin || current.id == author_id) {
        return layout::forbidden(&ctx).await;
    }

    if ctx.csrf_ok(&form.csrf) {
        // Comme l'original : le dernier commentaire disparaît, les autres laissent la
        // mention « Message supprimé » pour ne pas décaler la discussion.
        let last: Option<u32> = sqlx::query_scalar(
            "SELECT id FROM nf_comment WHERE module = ? AND module_id = ? ORDER BY id DESC LIMIT 1",
        )
        .bind(&module)
        .bind(module_id)
        .fetch_optional(db)
        .await?;
        if last == Some(id) {
            sqlx::query("DELETE FROM nf_comment WHERE id = ?").bind(id).execute(db).await?;
        } else {
            sqlx::query("UPDATE nf_comment SET content = '' WHERE id = ?").bind(id).execute(db).await?;
        }
    }

    let path = target_path(&ctx, &module, module_id).await?.unwrap_or_default();
    Ok(Redirect::to(&format!("{}#comments", ctx.url(&path))).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_are_trimmed_encoded_and_bounded() {
        assert_eq!(prepare("  salut <b>toi</b>\r\nligne 2 "), Some("salut &lt;b&gt;toi&lt;/b&gt;\nligne 2".to_owned()));
        assert_eq!(prepare("   \n "), None);
        assert_eq!(prepare(&"x".repeat(MAX_LENGTH + 1)), None);
        assert!(prepare(&"x".repeat(MAX_LENGTH)).is_some());
    }
}
