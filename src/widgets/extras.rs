//! Widgets « À propos », « Réseaux sociaux » et « Slider ».

use crate::{
    core::{
        bbcode,
        ctx::Ctx,
        panel::{Block, Panel},
        php::Value,
        text,
    },
    error::AppError,
};

fn yes(settings: &Value, key: &str, default: bool) -> bool {
    match settings.get_str(key) {
        Some("oui") => true,
        Some("non") => false,
        _ => default,
    }
}

fn align(settings: &Value, key: &str) -> &'static str {
    match settings.get_str(key) {
        Some("text-center") => "text-center",
        Some("text-right") => "text-right",
        _ => "text-left",
    }
}

/// `padding_*` et `margin_*` des réglages, en style CSS (valeurs numériques bornées).
fn spacing(settings: &Value) -> String {
    let mut css = String::new();
    for property in ["padding", "margin"] {
        for side in ["top", "right", "bottom", "left"] {
            let px = settings.get(&format!("{property}_{side}")).and_then(Value::as_i64).unwrap_or(0).clamp(0, 500);
            if px > 0 {
                css.push_str(&format!("{property}-{side}:{px}px;"));
            }
        }
    }
    css
}

/// Couleur hexadécimale (`#abc` ou `#aabbcc`) en attribut `style`, sinon rien.
fn color_attr(settings: &Value, key: &str) -> String {
    let value = settings.get_str(key).unwrap_or("");
    let hex = value.strip_prefix('#').unwrap_or("");
    if matches!(hex.len(), 3 | 6) && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        format!(" style=\"color:{value}\"")
    } else {
        String::new()
    }
}

fn wrap(settings: &Value, default_panel: bool, html: String) -> Vec<Block> {
    if yes(settings, "display_panel", default_panel) {
        vec![Block::Panel(Panel::new().body(html))]
    } else {
        vec![Block::Html(html)]
    }
}

/// Présentation de l'équipe (réglages `nf_team_*`).
pub async fn about(ctx: &Ctx, settings: &Value) -> Result<Vec<Block>, AppError> {
    let config = &ctx.settings;
    let mut html = format!("<div class=\"about-content\" style=\"{}\">", spacing(settings));
    if yes(settings, "display_teamname", true) {
        let title_color = color_attr(settings, "style_title");
        let title_align = align(settings, "teamname_align");
        let name = text::clean(config.get("nf_team_name"));
        if !name.is_empty() {
            html.push_str(&format!("<h3 class=\"{title_align}\"{title_color}>{name}</h3>"));
        }
        let mut items = String::new();
        let kind = text::clean(config.get("nf_team_type"));
        if yes(settings, "display_type", true) && !kind.is_empty() {
            items.push_str(&format!("<li class=\"list-inline-item\">{} {kind}</li>", text::icon("fas fa-university")));
        }
        let creation = text::clean(config.get("nf_team_creation"));
        if yes(settings, "display_date", true) && !creation.is_empty() {
            let shown = chrono::NaiveDate::parse_from_str(&creation, "%Y-%m-%d")
                .map(|d| d.format("%d/%m/%Y").to_string())
                .unwrap_or(creation);
            items.push_str(&format!("<li class=\"list-inline-item\">{} {shown}</li>", text::icon("fas fa-calendar-alt")));
        }
        if !items.is_empty() {
            html.push_str(&format!("<ul class=\"list-inline {title_align}\"{title_color}>{items}</ul>"));
        }
    }
    let logo_id = config.int("nf_team_logo");
    if yes(settings, "display_logo", true) && logo_id > 0 {
        let path: Option<String> = sqlx::query_scalar("SELECT path FROM nf_file WHERE id = ?")
            .bind(logo_id)
            .fetch_optional(&ctx.state.db)
            .await?;
        if let Some(path) = path {
            let width = settings.get("logo_width").and_then(Value::as_i64).unwrap_or(200).clamp(16, 2000);
            html.push_str(&format!(
                "<div class=\"{} mb-3\"><img src=\"{}\" style=\"max-width:{width}px\" alt=\"\" /></div>",
                align(settings, "logo_align"),
                text::encode(&ctx.url(path.trim_start_matches("./"))),
            ));
        }
    }
    let biography = config.get("nf_team_biographie");
    if yes(settings, "display_biographie", true) && !biography.is_empty() {
        html.push_str(&format!(
            "<div class=\"{}\"{}>{}</div>",
            align(settings, "biographie_align"),
            color_attr(settings, "style_text"),
            bbcode::render(&text::clean(biography)),
        ));
    }
    html.push_str("</div>");
    Ok(wrap(settings, true, html))
}

/// Réseaux gérés : réglage, nom, icône, couleur.
pub const SOCIALS: [(&str, &str, &str, &str); 12] = [
    ("nf_social_facebook", "Facebook", "fa-facebook-f", "#3B5998"),
    ("nf_social_twitter", "Twitter", "fa-twitter", "#55ACEE"),
    ("nf_social_google", "Google+", "fa-google-plus-g", "#dd4b39"),
    ("nf_social_steam", "Steam", "fa-steam", "#00adee"),
    ("nf_social_twitch", "Twitch", "fa-twitch", "#772ce8"),
    ("nf_social_dribble", "Dribbble", "fa-dribbble", "#ea4c89"),
    ("nf_social_behance", "Behance", "fa-behance", "#2196f3"),
    ("nf_social_deviantart", "DeviantArt", "fa-deviantart", "#00e59b"),
    ("nf_social_flickr", "Flickr", "fa-flickr", "#f40083"),
    ("nf_social_github", "GitHub", "fa-github", "#24292e"),
    ("nf_social_instagram", "Instagram", "fa-instagram", "#125688"),
    ("nf_social_youtube", "Youtube", "fa-youtube", "#bb0000"),
];

fn one_of<'a>(settings: &Value, key: &str, allowed: &[&'a str], default: &'a str) -> &'a str {
    let value = settings.get_str(key).unwrap_or("");
    allowed.iter().copied().find(|a| *a == value).unwrap_or(default)
}

/// Liens vers les réseaux sociaux renseignés dans les paramètres.
pub fn socials(ctx: &Ctx, settings: &Value) -> Vec<Block> {
    let display = one_of(settings, "social_display", &["col-12", "col-6", "col-4", "col-3", "col-2", "col-1", "col", "ul-inline", "ul"], "col-12");
    let style = one_of(settings, "social_style", &["btn btn-social btn-sm", "btn btn-social", "btn btn-social btn-lg", "btn btn-link"], "btn btn-social");
    let content = one_of(settings, "content_display", &["all", "icon", "legend"], "all");
    let size = one_of(settings, "icon_size", &["fa-1x", "fa-2x", "fa-3x", "fa-4x"], "fa-1x");
    let is_list = display.starts_with("ul");

    let mut buttons = String::new();
    for (setting, title, icon, color) in SOCIALS {
        let url = text::decode(ctx.settings.get(setting));
        // Seuls les liens web sont affichés (jamais `javascript:` ou autre).
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            continue;
        }
        let icon_html = if content == "legend" { String::new() } else { format!("<i class=\"fab {icon} {size}\"></i> ") };
        let label = if content == "icon" { String::new() } else { title.to_owned() };
        let button = format!(
            "<a href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\" class=\"{style}{}\" title=\"{title}\"{}>{icon_html}{label}</a>",
            text::encode(&url),
            if is_list { "" } else { " btn-block" },
            if style == "btn btn-link" { String::new() } else { format!(" style=\"background-color:{color};color:#fff\"") },
        );
        buttons.push_str(&match display {
            "ul-inline" => format!("<li class=\"list-inline-item\">{button}</li>"),
            "ul" => format!("<li>{button}</li>"),
            col => format!("<div class=\"{col} mb-2\">{button}</div>"),
        });
    }
    if buttons.is_empty() {
        return Vec::new();
    }
    let inner = match display {
        "ul-inline" => format!("<ul class=\"list-inline mb-0\">{buttons}</ul>"),
        "ul" => format!("<ul class=\"list-unstyled mb-0\">{buttons}</ul>"),
        _ => format!("<div class=\"row\">{buttons}</div>"),
    };
    wrap(settings, false, format!("<div class=\"socials-content\" style=\"{}\">{inner}</div>", spacing(settings)))
}

/// Carrousel d'accueil : comme l'original, deux diapositives fixes du thème.
pub fn slider(ctx: &Ctx) -> Vec<Block> {
    let name = text::clean(ctx.settings.get("nf_name"));
    let description = text::clean(ctx.settings.get("nf_description"));
    let slide = |n: u8, active: &str| {
        format!(
            "<div class=\"carousel-item{active}\"><img class=\"d-block w-100\" src=\"{}\" alt=\"\" />\
             <div class=\"carousel-caption d-none d-md-block\"><h3>{name}</h3><p>{description}</p></div></div>",
            ctx.url(&format!("images/slide{n}.jpg"))
        )
    };
    vec![Block::Html(format!(
        "<div id=\"carousel\" class=\"carousel slide mb-4\" data-ride=\"carousel\">\
         <ol class=\"carousel-indicators\"><li data-target=\"#carousel\" data-slide-to=\"0\" class=\"active\"></li><li data-target=\"#carousel\" data-slide-to=\"1\"></li></ol>\
         <div class=\"carousel-inner\">{}{}</div>\
         <a class=\"carousel-control-prev\" href=\"#carousel\" role=\"button\" data-slide=\"prev\"><span class=\"carousel-control-prev-icon\" aria-hidden=\"true\"></span><span class=\"sr-only\">Précédent</span></a>\
         <a class=\"carousel-control-next\" href=\"#carousel\" role=\"button\" data-slide=\"next\"><span class=\"carousel-control-next-icon\" aria-hidden=\"true\"></span><span class=\"sr-only\">Suivant</span></a></div>",
        slide(1, " active"),
        slide(2, ""),
    ))]
}
