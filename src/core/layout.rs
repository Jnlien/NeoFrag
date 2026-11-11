//! Assemblage d'une page : zones du thème, dispositions et widgets.
//!
//! Porté de `Output::zone()` et des « displayables » `Row` / `Col` / `Widget`. Les
//! dispositions sont lues telles que NeoFrag les a enregistrées (objets PHP sérialisés
//! dans `nf_dispositions`), ce qui permet de reprendre la mise en page d'un site existant.

use super::{ctx::Ctx, panel, php::{self, Value}, text};
use crate::{error::AppError, widgets};
use askama::Template;
use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use sqlx::Row;
use std::collections::HashMap;

/// Nombre de zones du thème Azuro : Haut, Entête, Menu, Slider, Avant-contenu,
/// Contenu, Post-contenu, Pied de page.
pub const ZONES: usize = 8;
const CONTENT_ZONE: usize = 5;

pub struct Crumb {
    /// HTML déjà encodé.
    pub title: String,
    pub url: String,
}

/// Ce qu'un module renvoie pour être habillé par le thème.
pub struct Page {
    /// Titre de la page (HTML encodé), vide pour reprendre celui du module.
    pub title: String,
    /// Nom technique du module (`news`, `pages`…), utilisé pour les classes CSS.
    pub module: &'static str,
    pub module_title: &'static str,
    pub module_icon: &'static str,
    /// Nom de l'action dans l'original (`index`, `_news`…). Les dispositions sont
    /// enregistrées par module/action (`news/_news/*`), pas par URL.
    pub method: &'static str,
    /// Fil d'Ariane, après l'entrée du module.
    pub breadcrumb: Vec<Crumb>,
    pub content: String,
    pub status: StatusCode,
}

impl Page {
    pub fn new(module: &'static str, module_title: &'static str, module_icon: &'static str) -> Self {
        Self {
            title: String::new(),
            module,
            module_title,
            module_icon,
            method: "index",
            breadcrumb: Vec::new(),
            content: String::new(),
            status: StatusCode::OK,
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn method(mut self, method: &'static str) -> Self {
        self.method = method;
        self
    }

    pub fn crumb(mut self, title: impl Into<String>, url: impl Into<String>) -> Self {
        self.breadcrumb.push(Crumb {
            title: title.into(),
            url: url.into(),
        });
        self
    }

    pub fn content(mut self, html: impl Into<String>) -> Self {
        self.content = html.into();
        self
    }

    pub fn blocks(self, blocks: &[panel::Block]) -> Self {
        self.content(panel::render_blocks(blocks))
    }
}

#[derive(Template)]
#[template(path = "azuro.html")]
struct AzuroTemplate<'a> {
    lang: &'a str,
    base: &'a str,
    title: String,
    description: String,
    zones: Vec<String>,
    breadcrumb: String,
    module: &'a str,
}

/// Sélectionne, pour chaque zone, la disposition la plus spécifique à la page courante.
async fn dispositions(ctx: &Ctx, page: &Page, theme: &str) -> Result<HashMap<usize, String>, AppError> {
    // Du plus général au plus précis ; le tri SQL (`page DESC`) fait gagner le plus précis.
    let mut pages = vec!["*".to_owned()];
    if ctx.path.is_empty() {
        pages.push("/".to_owned());
    }
    pages.push(format!("{}/*", page.module));
    pages.push(format!("{}/{}/*", page.module, page.method));

    let placeholders = vec!["?"; pages.len()].join(", ");
    let sql = format!(
        "SELECT zone, disposition FROM nf_dispositions WHERE theme = ? AND page IN ({placeholders}) ORDER BY page DESC"
    );
    let mut query = sqlx::query(&sql).bind(theme);
    for page in &pages {
        query = query.bind(page);
    }

    let mut by_zone = HashMap::new();
    for row in query.fetch_all(&ctx.state.db).await? {
        let zone = row.try_get::<u32, _>("zone")? as usize;
        by_zone.entry(zone).or_insert(row.try_get::<String, _>("disposition")?);
    }
    Ok(by_zone)
}

fn class_is(value: &Value, suffix: &str) -> bool {
    value.class().is_some_and(|class| class.ends_with(suffix))
}

/// `col-7` → `col-12 col-lg-7`, comme `Col::__toString()`.
fn col_class(size: &str) -> String {
    let size = text::css_classes(size);
    match size.strip_prefix("col-").and_then(|n| n.parse::<u8>().ok()) {
        Some(n) if n < 12 => format!("col-12 col-lg-{n}"),
        _ if size.is_empty() => "col-12".to_owned(),
        _ => size,
    }
}

async fn render_widget(ctx: &Ctx, page: &Page, node: &Value) -> Result<String, AppError> {
    let Some(widget_id) = node.get("_widget").and_then(Value::as_i64) else {
        return Ok(String::new());
    };
    let row = sqlx::query("SELECT widget, type, title, settings FROM nf_widgets WHERE widget_id = ?")
        .bind(widget_id)
        .fetch_optional(&ctx.state.db)
        .await?;
    let Some(row) = row else {
        return Ok(String::new());
    };

    let name: String = row.try_get("widget")?;
    let kind: String = row.try_get("type")?;
    let title: Option<String> = row.try_get("title")?;
    let settings = row
        .try_get::<Option<String>, _>("settings")?
        .and_then(|raw| php::unserialize(&raw).ok())
        .filter(|value| matches!(value, Value::Array(_)))
        .unwrap_or(Value::Array(Vec::new()));

    let Some(mut blocks) = widgets::render(ctx, page, &name, &kind, &settings).await? else {
        return Ok(String::new());
    };

    let style = node.get_str("_style").unwrap_or("");
    for block in &mut blocks {
        if let panel::Block::Panel(panel) = block {
            if let Some(title) = title.as_deref().filter(|t| !t.is_empty()) {
                panel.set_title(title);
            }
            if !style.is_empty() {
                panel.set_style(style);
            }
        }
    }

    let (kind_class, name_class) = if name == "module" {
        ("module", page.module)
    } else {
        ("widget", name.as_str())
    };
    Ok(format!(
        "<div class=\"{kind_class} {kind_class}-{}\">{}</div>",
        text::css_classes(name_class),
        panel::render_blocks(&blocks)
    ))
}

async fn render_zone(ctx: &Ctx, page: &Page, raw: &str) -> Result<String, AppError> {
    let Ok(root) = php::unserialize(raw) else {
        tracing::warn!("disposition illisible, zone ignorée");
        return Ok(String::new());
    };
    let Some(rows) = root.get("_array") else {
        return Ok(String::new());
    };

    let mut out = String::new();
    for row in rows.items() {
        if !class_is(row, "\\Row") {
            continue;
        }
        let mut cols_html = String::new();
        for col in row.get("_array").map(Value::items).unwrap_or_default() {
            if !class_is(col, "\\Col") {
                continue;
            }
            let children = col.get("_array").map(Value::items).unwrap_or_default();
            // Une colonne sans taille prend celle de son premier widget qui en a une.
            let size = col
                .get_str("_size")
                .filter(|s| !s.is_empty())
                .or_else(|| children.iter().find_map(|w| w.get_str("_size").filter(|s| !s.is_empty())))
                .unwrap_or("");

            let mut inner = String::new();
            for child in children {
                if class_is(child, "\\Widget") {
                    inner.push_str(&render_widget(ctx, page, child).await?);
                }
            }
            cols_html.push_str(&format!("<div class=\"{}\">{inner}</div>", col_class(size)));
        }

        let style = text::css_classes(row.get_str("_style").unwrap_or(""));
        let sep = if style.is_empty() { "" } else { " " };
        out.push_str(&format!("<div class=\"row{sep}{style}\">{cols_html}</div>"));
    }
    Ok(out)
}

/// Habille le contenu d'un module avec le thème et renvoie la réponse HTTP.
pub async fn render(ctx: &Ctx, page: Page) -> Result<Response, AppError> {
    let theme = match ctx.settings.get("nf_default_theme") {
        "" => "azuro",
        theme => theme,
    };
    let dispositions = dispositions(ctx, &page, theme).await?;

    let mut zones = Vec::with_capacity(ZONES);
    for zone in 0..ZONES {
        let html = match dispositions.get(&zone) {
            Some(raw) => render_zone(ctx, &page, raw).await?,
            None => String::new(),
        };
        zones.push(html);
    }

    // Base sans disposition pour ce thème (ou zone de contenu vide) : on affiche quand
    // même le module plutôt qu'une page blanche.
    if zones[CONTENT_ZONE].is_empty() && !page.content.is_empty() {
        zones[CONTENT_ZONE] = format!(
            "<div class=\"row\"><div class=\"col-12\"><div class=\"module module-{}\">{}</div></div></div>",
            page.module, page.content
        );
    }

    let breadcrumb = if ctx.path.is_empty() {
        String::new()
    } else {
        widgets::breadcrumb::html(ctx, &page)
    };

    let site_name = ctx.settings.get("nf_name");
    let page_title = if page.title.is_empty() { page.module_title } else { &page.title };
    let title = if page_title.is_empty() {
        site_name.to_owned()
    } else {
        format!("{page_title} :: {site_name}")
    };

    let html = AzuroTemplate {
        lang: ctx.lang(),
        base: ctx.base(),
        title,
        description: ctx.settings.get("nf_description").to_owned(),
        zones,
        breadcrumb,
        module: page.module,
    }
    .render()?;

    Ok((page.status, Html(html)).into_response())
}

fn error_page(status: StatusCode, title: &'static str, message: &str, icon: &'static str) -> Page {
    let panel = panel::Panel::new()
        .heading(title, icon)
        .style("card-danger")
        .body(format!("<div class=\"text-center\">{message}</div>"));
    let mut page = Page::new("error", title, icon).blocks(&[panel::Block::Panel(panel)]);
    page.status = status;
    page
}

pub async fn not_found(ctx: &Ctx) -> Result<Response, AppError> {
    let page = error_page(
        StatusCode::NOT_FOUND,
        "Page introuvable",
        "La page que vous souhaitez consulter est introuvable.",
        "fas fa-exclamation-triangle",
    );
    render(ctx, page).await
}

pub async fn forbidden(ctx: &Ctx) -> Result<Response, AppError> {
    let message = if ctx.user.is_some() {
        "Vous n'êtes pas autorisé à accéder à cette page.".to_owned()
    } else {
        format!(
            "Vous devez être connecté pour accéder à cette page. <a href=\"{}\">Se connecter</a>",
            ctx.url("user/login")
        )
    };
    let page = error_page(StatusCode::FORBIDDEN, "Accès refusé", &message, "fas fa-lock");
    render(ctx, page).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_sizes() {
        assert_eq!(col_class("col-7"), "col-12 col-lg-7");
        assert_eq!(col_class("col-12"), "col-12");
        assert_eq!(col_class(""), "col-12");
        assert_eq!(col_class("col-md-4"), "col-md-4");
        assert_eq!(col_class("col-3\" onload=\"x"), "col-3 onloadx");
    }
}
