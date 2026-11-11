//! Helpers de texte portés de `neofrag/helpers/string.php` et `assets.php`.

use chrono::{Datelike, NaiveDateTime, Timelike};
use regex::Regex;
use std::sync::LazyLock;

/// NeoFrag enregistre le texte saisi déjà encodé en entités HTML, puis l'affiche tel quel.
/// On garde cette convention pour rester compatible avec les données existantes.
pub fn encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#039;"),
            _ => out.push(c),
        }
    }
    out
}

/// Inverse de [`encode`] (et de `htmlentities` : gère aussi `&eacute;` & co).
pub fn decode(input: &str) -> String {
    html_escape::decode_html_entities(input).into_owned()
}

/// Normalise un texte lu en base avant de l'afficher : quel que soit son état
/// (encodé par NeoFrag, ou brut dans une vieille donnée), le résultat est sûr en HTML
/// et s'affiche à l'identique.
pub fn clean(input: &str) -> String {
    encode(&decode(input))
}

fn strip_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for c in input.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// Slug d'URL, équivalent de `url_title()`.
pub fn url_title(input: &str) -> String {
    let text = strip_tags(&decode(input));
    let mut ascii = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => ascii.push('a'),
            'Æ' | 'æ' => ascii.push_str("ae"),
            'Ç' | 'ç' => ascii.push('c'),
            'È' | 'É' | 'Ê' | 'Ë' | 'è' | 'é' | 'ê' | 'ë' => ascii.push('e'),
            'Ì' | 'Í' | 'Î' | 'Ï' | 'ì' | 'í' | 'î' | 'ï' => ascii.push('i'),
            'Ñ' | 'ñ' => ascii.push('n'),
            'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'Ø' | 'ø' => ascii.push('o'),
            'Œ' | 'œ' => ascii.push_str("oe"),
            'Ù' | 'Ú' | 'Û' | 'Ü' | 'ù' | 'ú' | 'û' | 'ü' => ascii.push('u'),
            'Ý' | 'ý' | 'ÿ' => ascii.push('y'),
            'ß' => ascii.push_str("ss"),
            _ => ascii.extend(c.to_lowercase()),
        }
    }

    let mut slug = String::with_capacity(ascii.len());
    let mut pending_dash = false;
    for c in ascii.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.push(c);
        } else {
            pending_dash = true;
        }
    }
    slug
}

/// Balise d'icône Font Awesome, équivalent de `icon()`.
pub fn icon(name: &str) -> String {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == ' ');
    if valid && (name.starts_with("fa") || name.starts_with("pe-7s-")) {
        format!("<i class=\"icon {name} fa-fw\"></i>")
    } else {
        String::new()
    }
}

/// Garde-fou pour les classes CSS lues en base (`_style`, `_size`, `align`…).
pub fn css_classes(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' '))
        .collect()
}

/// Garde-fou pour les couleurs CSS lues en base.
pub fn css_color(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '#' | '(' | ')' | ',' | '.' | '%' | ' '))
        .collect()
}

const MONTHS_SHORT: [&str; 12] = [
    "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc.",
];

/// `timetostr('%e %b %Y', $date)` en français.
/// Date et heure : « 11 nov. 2026 à 14:05 ».
pub fn date_time(date: &NaiveDateTime) -> String {
    format!("{} à {:02}:{:02}", date_short(date), date.hour(), date.minute())
}

static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://[^\s<]+").expect("motif valide"));

/// Rend cliquables les adresses web d'un texte déjà encodé (`strtolink()` de l'original).
pub fn linkify(encoded: &str) -> String {
    URL.replace_all(encoded, |caps: &regex::Captures| {
        let mut url = &caps[0];
        let mut tail = String::new();
        // La ponctuation et les guillemets (encodés) qui suivent ne font pas partie de l'adresse.
        loop {
            let cut = ["&quot;", "&#039;", "&gt;", "&lt;", ".", ",", ";", ":", "!", "?", ")"]
                .iter()
                .find(|suffix| url.ends_with(*suffix));
            match cut {
                Some(suffix) => {
                    url = &url[..url.len() - suffix.len()];
                    tail.insert_str(0, suffix);
                }
                None => break,
            }
        }
        if url.len() <= "https://".len() {
            return caps[0].to_owned();
        }
        format!("<a href=\"{url}\" rel=\"nofollow noopener\" target=\"_blank\">{url}</a>{tail}")
    })
    .into_owned()
}

pub fn date_short(date: &NaiveDateTime) -> String {
    format!(
        "{} {} {}",
        date.day(),
        MONTHS_SHORT[date.month0() as usize],
        date.year()
    )
}

/// `str_shortener()` : coupe proprement un texte (déjà encodé) à `max` caractères.
pub fn shorten(input: &str, max: usize) -> String {
    let plain = decode(input);
    if plain.chars().count() <= max {
        return input.to_owned();
    }
    let cut: String = plain.chars().take(max.saturating_sub(1)).collect();
    format!("{}&hellip;", encode(cut.trim_end()))
}

/// Construit une URL interne à partir d'un chemin relatif (`news/3/titre`).
pub fn url(base: &str, path: &str) -> String {
    let path = path.trim_start_matches('/');
    if path.starts_with("http://") || path.starts_with("https://") || path.starts_with('#') {
        return path.to_owned();
    }
    let base = base.trim_end_matches('/');
    if path.is_empty() && !base.is_empty() {
        // Sous un préfixe, l'accueil est `/site` (et non `/site/`).
        return base.to_owned();
    }
    format!("{base}/{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(url_title("Bienvenue sur NeoFrag !"), "bienvenue-sur-neofrag");
        assert_eq!(url_title("Actualit&eacute;s d'&Eacute;t&eacute;"), "actualites-d-ete");
        assert_eq!(url_title("  --Œuvre_n°2--  "), "oeuvre-n-2");
        assert_eq!(url_title("<b>Gras</b>"), "gras");
        assert_eq!(url_title(""), "");
    }

    #[test]
    fn entities_roundtrip() {
        let raw = "<script>alert(\"x\") & 'y'</script>";
        let enc = encode(raw);
        assert!(!enc.contains('<') && !enc.contains('"'));
        assert_eq!(decode(&enc), raw);
    }

    #[test]
    fn clean_is_safe_and_idempotent() {
        assert_eq!(clean("G&eacute;n&eacute;ral"), "Général");
        assert_eq!(clean("<b>x</b>"), "&lt;b&gt;x&lt;/b&gt;");
        assert_eq!(clean(&clean("a &amp; <b>")), clean("a &amp; <b>"));
    }

    #[test]
    fn links_in_text() {
        assert_eq!(
            linkify("voir https://neofr.ag/doc?a=1&amp;b=2, merci"),
            "voir <a href=\"https://neofr.ag/doc?a=1&amp;b=2\" rel=\"nofollow noopener\" target=\"_blank\">https://neofr.ag/doc?a=1&amp;b=2</a>, merci"
        );
        // Guillemet encodé : il ne peut pas fermer l'attribut href.
        let html = linkify("&quot;http://a.fr/x&quot; onmouseover=&quot;y");
        assert!(html.contains("href=\"http://a.fr/x\""));
        assert!(!html.contains("href=\"http://a.fr/x&quot;"));
        assert_eq!(linkify("pas de lien : javascript:alert(1) ni https://"), "pas de lien : javascript:alert(1) ni https://");
    }

    #[test]
    fn icons_are_sanitized() {
        assert_eq!(icon("fas fa-user"), "<i class=\"icon fas fa-user fa-fw\"></i>");
        assert_eq!(icon("fas fa-user\"><script>"), "");
        assert_eq!(icon(""), "");
    }

    #[test]
    fn urls() {
        assert_eq!(url("", "news"), "/news");
        assert_eq!(url("/site/", "/news/1"), "/site/news/1");
        assert_eq!(url("", ""), "/");
        assert_eq!(url("/site", ""), "/site");
        assert_eq!(url("", "#"), "#");
    }

    #[test]
    fn shorten_keeps_short_text() {
        assert_eq!(shorten("court", 45), "court");
        assert!(shorten(&"a".repeat(60), 45).ends_with("&hellip;"));
    }
}
