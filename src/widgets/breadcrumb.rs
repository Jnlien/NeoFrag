//! Fil d'Ariane, porté de `widgets/breadcrumb`.

use crate::core::{ctx::Ctx, layout::Page, text};

pub fn html(ctx: &Ctx, page: &Page) -> String {
    // (titre, url, icône)
    let mut links: Vec<(String, String, &str)> = Vec::new();
    if ctx.path.is_empty() && page.breadcrumb.is_empty() {
        links.push(("Accueil".to_owned(), String::new(), "fas fa-map-marker-alt"));
    } else if !page.module_title.is_empty() && page.module != "pages" && page.module != "error" {
        // Une page statique porte son propre titre : pas d'entrée « module » devant.
        let icon = if page.module_icon.is_empty() { "fas fa-map-marker-alt" } else { page.module_icon };
        links.push((page.module_title.to_owned(), page.module.to_owned(), icon));
    }
    for crumb in &page.breadcrumb {
        links.push((crumb.title.clone(), crumb.url.clone(), page.module_icon));
    }

    let last = links.len().saturating_sub(1);
    let items: String = links
        .iter()
        .enumerate()
        .map(|(i, (title, url, icon))| {
            let is_last = i == last;
            format!(
                "<li class=\"breadcrumb-item{}\"><a href=\"{}\">{}{}</a></li>",
                if is_last { " active" } else { "" },
                text::encode(&ctx.url(url)),
                if is_last && !icon.is_empty() { format!("{} ", text::icon(icon)) } else { String::new() },
                title
            )
        })
        .collect();

    format!(
        "<ol class=\"breadcrumb\"><li class=\"breadcrumb-item\"><b>{}</b></li>{items}</ol>",
        ctx.settings.get("nf_name")
    )
}
