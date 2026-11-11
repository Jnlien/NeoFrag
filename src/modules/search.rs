//! Module Recherche : actualités et pages. Version simplifiée de `modules/search`
//! (l'original interroge chaque module ; ici on couvre ceux qui sont portés).

use crate::{
    core::{
        ctx::Ctx,
        layout::{self, Page},
        panel::{Block, Panel},
        text,
    },
    error::AppError,
};
use axum::{extract::Query, response::Response, Extension};
use serde::Deserialize;
use sqlx::Row;
use std::sync::Arc;

pub const TITLE: &str = "Rechercher";
pub const ICON: &str = "fas fa-search";

#[derive(Deserialize)]
pub struct Params {
    #[serde(default)]
    q: String,
}

/// Échappe les jokers de `LIKE` : l'utilisateur cherche du texte, pas un motif.
fn like_pattern(term: &str) -> String {
    let escaped = term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{escaped}%")
}

pub async fn index(Extension(ctx): Extension<Arc<Ctx>>, Query(params): Query<Params>) -> Result<Response, AppError> {
    let query: String = params.q.trim().chars().take(100).collect();
    let shown = text::encode(&query);

    let form = format!(
        "<form action=\"{}\" method=\"get\"><div class=\"input-group\">\
         <input type=\"text\" class=\"form-control\" name=\"q\" value=\"{shown}\" placeholder=\"Rechercher...\" />\
         <span class=\"input-group-append\"><button class=\"btn btn-primary\" type=\"submit\">{}</button></span>\
         </div></form>",
        ctx.url("search"),
        text::icon(ICON)
    );
    let mut blocks = vec![Block::Panel(Panel::new().heading(TITLE, ICON).body(form))];

    if query.chars().count() >= 3 {
        // Le texte est stocké encodé en entités : on cherche la forme encodée.
        let pattern = like_pattern(&shown);
        let db = &ctx.state.db;

        let news = sqlx::query(
            "SELECT n.news_id, nl.title FROM nf_news n \
             INNER JOIN nf_news_lang nl ON n.news_id = nl.news_id AND nl.lang = ? \
             WHERE n.published = '1' AND (nl.title LIKE ? OR nl.introduction LIKE ? OR nl.content LIKE ? OR nl.tags LIKE ?) \
             ORDER BY n.date DESC LIMIT 20",
        )
        .bind(ctx.lang())
        .bind(&pattern)
        .bind(&pattern)
        .bind(&pattern)
        .bind(&pattern)
        .fetch_all(db)
        .await?;

        let pages = sqlx::query(
            "SELECT p.page_id, p.name, pl.title FROM nf_pages p \
             INNER JOIN nf_pages_lang pl ON p.page_id = pl.page_id AND pl.lang = ? \
             WHERE p.published = '1' AND (pl.title LIKE ? OR pl.subtitle LIKE ? OR pl.content LIKE ?) \
             ORDER BY pl.title LIMIT 20",
        )
        .bind(ctx.lang())
        .bind(&pattern)
        .bind(&pattern)
        .bind(&pattern)
        .fetch_all(db)
        .await?;

        let mut results = String::new();
        for row in &news {
            let id: u32 = row.try_get("news_id")?;
            let title = text::clean(&row.try_get::<String, _>("title")?);
            results.push_str(&format!(
                "<li class=\"list-group-item\">{} <a href=\"{}\">{title}</a> <span class=\"badge badge-dark float-right\">Actualité</span></li>",
                text::icon("far fa-file-alt"),
                ctx.url(&format!("news/{id}/{}", text::url_title(&title)))
            ));
        }
        for row in &pages {
            let page_id: u32 = row.try_get("page_id")?;
            // Ne pas révéler l'existence d'une page à qui n'y a pas accès.
            if !ctx.access.can("pages", "access_page", page_id) {
                continue;
            }
            let name: String = row.try_get("name")?;
            let title = text::clean(&row.try_get::<String, _>("title")?);
            results.push_str(&format!(
                "<li class=\"list-group-item\">{} <a href=\"{}\">{title}</a> <span class=\"badge badge-secondary float-right\">Page</span></li>",
                text::icon("far fa-file"),
                text::encode(&ctx.url(&name))
            ));
        }

        let panel = Panel::new().heading(format!("Résultats pour « {shown} »"), "");
        blocks.push(Block::Panel(if results.is_empty() {
            panel.body("<div class=\"text-center\">Aucun résultat</div>")
        } else {
            panel.body_raw(format!("<ul class=\"list-group list-group-flush\">{results}</ul>"))
        }));
    } else if !query.is_empty() {
        blocks.push(Block::Panel(
            Panel::new().body("<div class=\"text-center\">Saisissez au moins 3 caractères.</div>"),
        ));
    }

    layout::render(&ctx, Page::new("search", TITLE, ICON).blocks(&blocks)).await
}

#[cfg(test)]
mod tests {
    use super::like_pattern;

    #[test]
    fn wildcards_are_escaped() {
        assert_eq!(like_pattern("100%_sûr"), "%100\\%\\_sûr%");
        assert_eq!(like_pattern("a\\b"), "%a\\\\b%");
    }
}
