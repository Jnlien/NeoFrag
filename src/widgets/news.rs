//! Widgets du module Actualités, portés de `widgets/news`.

use crate::{
    core::{
        ctx::Ctx,
        panel::{Block, Panel},
        text,
    },
    error::AppError,
    modules::news::{self, Filter},
};
use sqlx::Row;

const FOOTER_ICON: &str = "far fa-arrow-alt-circle-right";

/// Actualités récentes.
pub async fn index(ctx: &Ctx) -> Result<Vec<Block>, AppError> {
    let (items, _) = news::list(&ctx.state.db, ctx.lang(), &Filter::All, true, 3, 0).await?;
    let panel = Panel::new().heading("Actualités récentes", "");
    if items.is_empty() {
        return Ok(vec![Block::Panel(panel.body("Aucune actualité pour le moment"))]);
    }

    let mut body = String::new();
    for item in &items {
        body.push_str("<div class=\"media\">");
        if let Some(image) = item.image.as_deref().filter(|p| !p.is_empty()) {
            body.push_str(&format!(
                "<div class=\"media-left\"><div class=\"img-cover\" style=\"background-image: url('{}');\"></div></div>",
                text::encode(&ctx.url(image))
            ));
        }
        body.push_str(&format!(
            "<div class=\"media-body\"><a href=\"{}\" class=\"badge badge-dark\">{}</a><br /><a href=\"{}\">{}</a><br /></div></div>",
            text::encode(&ctx.url(&item.category_path())),
            item.category_title,
            ctx.url(&item.path()),
            item.title
        ));
    }
    let footer = format!(
        "<a href=\"{}\">{} Voir toutes les actualités</a>",
        ctx.url("news"),
        text::icon(FOOTER_ICON)
    );
    Ok(vec![Block::Panel(panel.body(body).footer(footer, true))])
}

pub async fn categories(ctx: &Ctx) -> Result<Vec<Block>, AppError> {
    let categories = news::categories(&ctx.state.db, ctx.lang()).await?;
    let panel = Panel::new().heading("Catégories", "");
    if categories.is_empty() {
        return Ok(vec![Block::Panel(panel.body("Aucune catégorie pour le moment"))]);
    }
    let items: String = categories
        .iter()
        .map(|c| {
            format!(
                "<li class=\"list-group-item\"><span class=\"badge badge-secondary float-right\">{}</span><a href=\"{}\">{}</a></li>",
                c.nb_news,
                text::encode(&ctx.url(&format!("news/category/{}/{}", c.category_id, c.name))),
                c.title
            )
        })
        .collect();
    Ok(vec![Block::Panel(
        panel.body_raw(format!("<ul class=\"list-group list-group-flush\">{items}</ul>")),
    )])
}

pub async fn tags(ctx: &Ctx) -> Result<Vec<Block>, AppError> {
    let rows = sqlx::query(
        "SELECT nl.tags FROM nf_news_lang nl INNER JOIN nf_news n ON nl.news_id = n.news_id \
         WHERE n.published = '1' AND nl.lang = ?",
    )
    .bind(ctx.lang())
    .fetch_all(&ctx.state.db)
    .await?;

    let mut tags: Vec<String> = Vec::new();
    for row in rows {
        let raw: String = row.try_get("tags")?;
        for tag in raw.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            if !tags.iter().any(|t| t == tag) {
                tags.push(tag.to_owned());
            }
        }
    }

    let panel = Panel::new().heading("Tags", "");
    if tags.is_empty() {
        return Ok(vec![Block::Panel(panel.body("Aucun tag pour le moment"))]);
    }
    let items: String = tags
        .iter()
        .map(|tag| {
            format!(
                "<li class=\"list-inline-item\"><a href=\"{}\">{}</a></li>",
                ctx.url(&format!("news/tag/{}", text::url_title(tag))),
                text::clean(tag)
            )
        })
        .collect();
    Ok(vec![Block::Panel(
        panel.body(format!("<ul class=\"list-inline mb-0\">{items}</ul>")),
    )])
}
