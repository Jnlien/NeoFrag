//! Module Membres : liste des inscrits. Porté de `modules/members`.

use crate::{
    core::{
        ctx::Ctx,
        layout::{self, Page},
        pagination::Pagination,
        panel::{Block, Panel},
        text, user,
    },
    error::AppError,
};
use axum::{
    extract::Path,
    response::{IntoResponse, Redirect, Response},
    Extension,
};
use chrono::NaiveDateTime;
use sqlx::Row;
use std::sync::Arc;

pub const TITLE: &str = "Membres";
pub const ICON: &str = "fas fa-users";
const PER_PAGE: i64 = 25;

async fn listing(ctx: &Ctx, page_no: u32) -> Result<Response, AppError> {
    let db = &ctx.state.db;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nf_user WHERE deleted = '0'")
        .fetch_one(db)
        .await?;
    let pagination = Pagination::new(page_no, PER_PAGE, total);
    if pagination.out_of_range() {
        return Ok(Redirect::to(&ctx.url("members")).into_response());
    }

    let rows = sqlx::query(
        "SELECT u.id, u.username, (u.admin = '1') AS admin, CAST(u.registration_date AS DATETIME) AS registration_date, f.path AS avatar, up.sex \
         FROM nf_user u \
         LEFT JOIN nf_user_profile up ON up.id = u.id \
         LEFT JOIN nf_file f ON f.id = up.avatar \
         WHERE u.deleted = '0' ORDER BY u.username LIMIT ? OFFSET ?",
    )
    .bind(pagination.per_page)
    .bind(pagination.offset())
    .fetch_all(db)
    .await?;

    let mut table = String::from(
        "<table class=\"table table-hover mb-0\"><thead><tr><th></th><th>Membre</th><th>Groupe</th><th>Inscription</th></tr></thead><tbody>",
    );
    for row in rows {
        let id: u32 = row.try_get("id")?;
        let username: String = row.try_get("username")?;
        let avatar: Option<String> = row.try_get("avatar")?;
        let sex: Option<String> = row.try_get("sex")?;
        let date: NaiveDateTime = row.try_get("registration_date")?;
        let group = if row.try_get::<i64, _>("admin")? != 0 {
            "<span class=\"badge badge-danger\">Administrateur</span>"
        } else {
            "<span class=\"badge badge-success\">Membre</span>"
        };
        table.push_str(&format!(
            "<tr><td class=\"col-1\">{}</td><td class=\"align-middle\">{}</td><td class=\"align-middle\">{group}</td><td class=\"align-middle\">{}</td></tr>",
            user::avatar_html(ctx.base(), avatar.as_deref(), sex.as_deref(), false, ""),
            user::link(ctx.base(), id, &username),
            text::date_short(&date)
        ));
    }
    table.push_str("</tbody></table>");

    let mut blocks = vec![Block::Panel(Panel::new().heading(TITLE, ICON).body_raw(table))];
    if let Some(panel) = pagination.panel(&ctx.url("members")) {
        blocks.push(Block::Panel(panel));
    }
    layout::render(ctx, Page::new("members", TITLE, ICON).blocks(&blocks)).await
}

pub async fn index(Extension(ctx): Extension<Arc<Ctx>>) -> Result<Response, AppError> {
    listing(&ctx, 1).await
}

pub async fn index_paged(Extension(ctx): Extension<Arc<Ctx>>, Path(page): Path<String>) -> Result<Response, AppError> {
    match page.parse::<u32>() {
        Ok(page) if page >= 1 => listing(&ctx, page).await,
        _ => layout::not_found(&ctx).await,
    }
}
