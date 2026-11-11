//! Widget Navigation (menus horizontaux et verticaux), porté de `widgets/navigation`.

use crate::core::{
    ctx::Ctx,
    panel::{Block, Panel},
    php::Value,
    text,
};

struct Link<'a> {
    title: &'a str,
    icon: &'a str,
    /// Cible simple, ou sous-menu si `children` n'est pas vide.
    url: &'a str,
    children: Vec<Link<'a>>,
}

fn parse(value: &Value) -> Link<'_> {
    let (url, children) = match value.get("url") {
        Some(Value::Str(url)) => (url.as_str(), Vec::new()),
        Some(list @ Value::Array(_)) => ("", list.items().into_iter().map(parse).collect()),
        _ => ("", Vec::new()),
    };
    Link {
        title: value.get_str("title").unwrap_or(""),
        icon: value.get_str("icon").unwrap_or(""),
        url,
        children,
    }
}

/// Le lien correspond-il à la page courante (ou à l'un de ses parents) ?
fn matches(path: &str, url: &str) -> bool {
    let url = url.trim_matches('/');
    url == path || (!url.is_empty() && path.starts_with(url) && path[url.len()..].starts_with('/'))
}

fn href(ctx: &Ctx, url: &str) -> String {
    text::encode(&ctx.url(url))
}

fn anchor(ctx: &Ctx, link: &Link, active: bool) -> String {
    format!(
        "<a class=\"nav-link{}\" href=\"{}\">{} {}</a>",
        if active { " active" } else { "" },
        href(ctx, link.url),
        text::icon(link.icon),
        text::clean(link.title)
    )
}

pub fn render(ctx: &Ctx, settings: &Value, vertical: bool) -> Vec<Block> {
    let links: Vec<Link> = settings
        .get("links")
        .map(Value::items)
        .unwrap_or_default()
        .into_iter()
        .map(parse)
        .collect();

    // Un seul lien actif : la cible la plus longue qui correspond à la page courante.
    let active = links
        .iter()
        .flat_map(|link| {
            if link.children.is_empty() {
                vec![link.url]
            } else {
                link.children.iter().map(|child| child.url).collect()
            }
        })
        .filter(|url| matches(&ctx.path, url))
        .max_by_key(|url| url.len());

    let mut nav = format!("<ul class=\"nav{}\">", if vertical { " flex-column" } else { "" });
    for link in &links {
        if link.children.is_empty() {
            nav.push_str(&format!(
                "<li class=\"nav-item\">{}</li>",
                anchor(ctx, link, active == Some(link.url))
            ));
            continue;
        }

        let any_active = link.children.iter().any(|child| active == Some(child.url));
        let submenu: String = link
            .children
            .iter()
            .map(|child| format!("<li class=\"nav-item\">{}</li>", anchor(ctx, child, active == Some(child.url))))
            .collect();
        nav.push_str(&format!(
            "<li class=\"nav-item\"><a class=\"nav-link{}\" data-toggle=\"collapse\" href=\"#\">{} \
             <span class=\"hidden-xs\">{}</span><span class=\"fas fa-angle-down\"></span></a>\
             <ul class=\"nav flex-column\">{submenu}</ul></li>",
            if any_active { " active" } else { "" },
            text::icon(link.icon),
            text::clean(link.title)
        ));
    }
    nav.push_str("</ul>");

    // Horizontal : dans un panel seulement si demandé ; vertical : dans un panel sauf refus.
    let in_panel = match settings.get("panel") {
        Some(value) => value.truthy(),
        None => vertical,
    };
    if in_panel {
        vec![Block::Panel(Panel::new().body_raw(nav))]
    } else {
        vec![Block::Html(nav)]
    }
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn active_link_detection() {
        assert!(matches("", ""));
        assert!(matches("news", "news"));
        assert!(matches("news/3/titre", "news"));
        assert!(!matches("newsletter", "news"));
        assert!(!matches("news", ""));
        assert!(matches("events/matches", "events/matches"));
    }
}
