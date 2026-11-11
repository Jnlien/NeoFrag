//! Widgets : les blocs que l'on place dans les zones du thème.
//!
//! Chaque widget est identifié par son nom (`nf_widgets.widget`) et un type
//! (`nf_widgets.type`), avec des réglages sérialisés. Un widget inconnu (module pas
//! encore porté) ne produit rien : la page reste utilisable.

pub mod breadcrumb;
pub mod extras;
pub mod members;
pub mod navigation;
pub mod news;
pub mod user;

use crate::{
    core::{
        bbcode,
        ctx::Ctx,
        layout::Page,
        panel::{Block, Panel},
        php::Value,
        text,
    },
    error::AppError,
    modules,
};
use chrono::Datelike;
use sqlx::Row;

pub async fn render(
    ctx: &Ctx,
    page: &Page,
    name: &str,
    kind: &str,
    settings: &Value,
) -> Result<Option<Vec<Block>>, AppError> {
    // Un widget rattaché à un module désactivé ne s'affiche pas.
    if !ctx.widget_enabled(name) {
        return Ok(None);
    }
    let blocks = match (name, kind) {
        ("module", _) => vec![Block::Html(page.content.clone())],
        ("header", _) => vec![Block::Html(header(ctx, settings).await?)],
        ("navigation", "vertical") => navigation::render(ctx, settings, true),
        ("navigation", _) => navigation::render(ctx, settings, false),
        ("breadcrumb", _) => vec![Block::Html(breadcrumb::html(ctx, page))],
        ("copyright", _) => vec![copyright(ctx)],
        ("html", "html") => vec![Block::Panel(
            // Type « html » : contenu saisi par un administrateur, affiché tel quel (comme l'original).
            Panel::new().body(text::decode(settings.get_str("content").unwrap_or(""))),
        )],
        ("html", _) => vec![Block::Panel(
            Panel::new().body(bbcode::render(settings.get_str("content").unwrap_or(""))),
        )],
        ("search", _) => vec![Block::Html(search(ctx, settings))],
        ("user", "index_mini") => vec![Block::Html(user::index_mini(ctx, settings))],
        ("user", "index") => user::index(ctx),
        ("members", "online_mini") => vec![Block::Html(members::online_mini(ctx, settings).await?)],
        ("members", "online") => members::online(ctx).await?,
        ("members", "index") => members::index(ctx).await?,
        ("news", "categories") => news::categories(ctx).await?,
        ("news", "tags") => news::tags(ctx).await?,
        ("news", "index") => news::index(ctx).await?,
        ("forum", kind) => modules::forum::widget(ctx, kind).await?,
        ("gallery", kind) => modules::gallery::widget(ctx, kind).await?,
        ("teams", _) => modules::teams::widget(ctx).await?,
        ("partners", "column") => modules::partners::widget(ctx, settings, true).await?,
        ("partners", _) => modules::partners::widget(ctx, settings, false).await?,
        ("awards", _) => modules::awards::widget(ctx).await?,
        ("about", _) => extras::about(ctx, settings).await?,
        ("socials", _) => extras::socials(ctx, settings),
        ("slider", _) => extras::slider(ctx),
        ("recruits", _) => modules::recruits::widget(ctx).await?,
        ("talks", _) => modules::talks::widget(ctx, settings).await?,
        ("events", kind) => modules::events::widget(ctx, kind).await?,
        _ => {
            tracing::debug!(widget = name, kind, "widget pas encore porté");
            return Ok(None);
        }
    };
    Ok(Some(blocks))
}

async fn header(ctx: &Ctx, settings: &Value) -> Result<String, AppError> {
    let align = text::css_classes(settings.get_str("align").unwrap_or(""));

    if settings.get_str("display") == Some("logo") {
        let logo_id = ctx.settings.int("nf_team_logo");
        if logo_id > 0 {
            let row = sqlx::query("SELECT path FROM nf_file WHERE id = ?")
                .bind(logo_id)
                .fetch_optional(&ctx.state.db)
                .await?;
            if let Some(row) = row {
                let path: String = row.try_get("path")?;
                return Ok(format!(
                    "<div class=\"{align}\"><a href=\"{}\"><img src=\"{}\" class=\"header-logo\" alt=\"\" /></a></div>",
                    ctx.url(""),
                    text::encode(&ctx.url(&path))
                ));
            }
        }
    }

    let style = |key: &str| match settings.get_str(key).map(text::css_color) {
        Some(color) if !color.is_empty() => format!(" style=\"color: {color}\""),
        _ => String::new(),
    };
    let or_setting = |key: &str, fallback: &str| match settings.get_str(key) {
        Some(value) if !value.is_empty() => value.to_owned(),
        _ => ctx.settings.get(fallback).to_owned(),
    };
    Ok(format!(
        "<div class=\"{align}\"><h2 class=\"site-title\"{}>{}</h2><h5 class=\"site-description\"{}>{}</h5></div>",
        style("color_title"),
        or_setting("title", "nf_name"),
        style("color_description"),
        or_setting("description", "nf_description"),
    ))
}

fn copyright(ctx: &Ctx) -> Block {
    let mut copyright = text::decode(ctx.settings.get("nf_copyright"));
    if !copyright.contains("{neofrag}") {
        copyright.push_str("<div class=\"float-right\">Propulsé par {neofrag}</div>");
    }
    let html = copyright
        .replace(
            "{name}",
            &format!("<a href=\"{}\">{}</a>", ctx.url(""), ctx.settings.get("nf_name")),
        )
        .replace("{neofrag}", "<a href=\"https://neofr.ag\">NeoFrag</a>")
        .replace("{year}", &chrono::Local::now().year().to_string())
        .replace("{copyright}", &text::icon("far fa-copyright"));
    Block::Panel(Panel::new().body(html))
}

fn search(ctx: &Ctx, settings: &Value) -> String {
    let align = match settings.get_str("align") {
        Some(align) if !align.is_empty() => text::css_classes(align),
        _ => "float-right".to_owned(),
    };
    format!(
        "<form class=\"form-inline {align}\" action=\"{}\" method=\"get\">\
         <div class=\"input-group input-group-sm\">\
         <input type=\"text\" class=\"form-control\" name=\"q\" placeholder=\"Rechercher...\" />\
         <span class=\"input-group-append\"><button class=\"btn btn-light\" type=\"submit\">{}</button></span>\
         </div></form>",
        ctx.url("search"),
        text::icon("fas fa-search")
    )
}
