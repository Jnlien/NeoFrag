//! Module Pages : pages statiques accessibles par leur nom (`/reglement`…).
//! Porté de `modules/pages`.

use crate::{
    core::{
        bbcode,
        ctx::Ctx,
        layout::{self, Page},
        panel::{Block, Panel},
        text,
    },
    error::AppError,
};
use axum::response::Response;
use sqlx::Row;

pub const TITLE: &str = "Pages";
pub const ICON: &str = "far fa-file-alt";

/// Noms de premier segment réservés aux modules et aux fichiers statiques : une page
/// ne peut pas les porter (elle serait inaccessible).
pub const RESERVED: &[&str] = &[
    "admin", "ajax", "api", "css", "fonts", "images", "index", "js", "members", "modules", "news", "pages",
    "search", "themes", "upload", "user", "widgets",
];

/// Affiche la page publiée nommée `name`, ou une 404.
pub async fn show(ctx: &Ctx, name: &str) -> Result<Response, AppError> {
    let row = sqlx::query(
        "SELECT p.page_id, pl.title, pl.subtitle, pl.content FROM nf_pages p \
         INNER JOIN nf_pages_lang pl ON p.page_id = pl.page_id AND pl.lang = ? \
         WHERE p.name = ? AND p.published = '1'",
    )
    .bind(ctx.lang())
    .bind(name)
    .fetch_optional(&ctx.state.db)
    .await?;

    let Some(row) = row else {
        return layout::not_found(ctx).await;
    };
    let page_id: u32 = row.try_get("page_id")?;
    if !ctx.access.can("pages", "access_page", page_id) {
        return layout::forbidden(ctx).await;
    }

    let title = text::clean(&row.try_get::<String, _>("title")?);
    let subtitle = text::clean(&row.try_get::<String, _>("subtitle")?);
    let content = text::clean(&row.try_get::<String, _>("content")?);

    let heading = if subtitle.is_empty() {
        title.clone()
    } else {
        format!("{title} <small>{subtitle}</small>")
    };
    let panel = Panel::new().heading(heading, ICON).body(bbcode::render(&content));

    let page = Page::new("pages", TITLE, ICON)
        .title(title.clone())
        .crumb(title, name.to_owned())
        .blocks(&[Block::Panel(panel)]);
    layout::render(ctx, page).await
}
