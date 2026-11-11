//! BBCode → HTML, porté de `neofrag/libraries/bbcode.php`.
//!
//! Le texte reçu est déjà encodé en entités (voir `text::encode`) : aucun `<` ni `"`
//! ne peut donc venir de l'utilisateur. Par rapport à l'original, les URL sont
//! filtrées (`javascript:` & co refusés).

use regex::{Captures, Regex};
use std::sync::LazyLock;

/// Remplace tant que le motif apparaît (les balises peuvent être imbriquées).
fn replace_loop(text: String, re: &Regex, rep: impl Fn(&Captures) -> String) -> String {
    let mut text = text;
    // Borne de sécurité : chaque passe consomme au moins une paire de balises.
    for _ in 0..64 {
        if !re.is_match(&text) {
            break;
        }
        text = re.replace_all(&text, &rep).into_owned();
    }
    text
}

fn re(pattern: &str) -> Regex {
    Regex::new(&format!("(?s){pattern}")).expect("motif BBCode valide")
}

fn safe_url(raw: &str) -> Option<String> {
    let url = raw.trim();
    let lower = url.to_ascii_lowercase();
    let ok = lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || lower.starts_with('/')
        || lower.starts_with('#')
        || !lower.contains(':');
    (ok && !url.is_empty() && !url.contains(['<', '>', '"', '\'', '\n', '\r'])).then(|| url.to_owned())
}

struct Rules {
    simple: Vec<(Regex, &'static str, &'static str)>,
    img: Regex,
    video: Regex,
    url: Regex,
    url_named: Regex,
    list: Regex,
    list_ordered: Regex,
    item: Regex,
    color: Regex,
    size: Regex,
    font: Regex,
    headings: Regex,
}

static RULES: LazyLock<Rules> = LazyLock::new(|| {
    let pair = |tag: &str, open: &'static str, close: &'static str| {
        (re(&format!(r"\[{tag}\](.*?)\[/{tag}\]")), open, close)
    };
    Rules {
        simple: vec![
            pair("b", "<b>", "</b>"),
            pair("bold", "<b>", "</b>"),
            pair("strong", "<b>", "</b>"),
            pair("i", "<i>", "</i>"),
            pair("italic", "<i>", "</i>"),
            pair("u", "<u>", "</u>"),
            pair("underline", "<u>", "</u>"),
            pair("s", "<strike>", "</strike>"),
            pair("strike", "<strike>", "</strike>"),
            pair("sup", "<sup>", "</sup>"),
            pair("sub", "<sub>", "</sub>"),
            pair("tr", "<tr>", "</tr>"),
            pair("td", "<td>", "</td>"),
            pair("left", "<div style=\"text-align: left;\">", "</div>"),
            pair("center", "<div style=\"text-align: center;\">", "</div>"),
            pair("right", "<div style=\"text-align: right;\">", "</div>"),
            pair("quote", "<blockquote>", "</blockquote>"),
            pair("code", "<code>", "</code>"),
            pair("table", "<table class=\"table table-bordered\">", "</table>"),
        ],
        img: re(r"\[img\](.*?)\[/img\]"),
        video: re(r"\[video\]([a-zA-Z0-9_-]{6,11})\[/video\]"),
        url: re(r"\[url\](.*?)\[/url\]"),
        url_named: re(r"\[url=(.*?)\](.*?)\[/url\]"),
        list: re(r"\[list\](.*?)\[/list\]"),
        list_ordered: re(r"\[list=1\](.*?)\[/list\]"),
        item: re(r"\[\*\](.*?)\[/\*\][\r\n]*"),
        color: re(r"\[color=(#(?:[0-9a-fA-F]{3}){1,2})\](.*?)\[/color\]"),
        size: re(r"\[size=(\d{1,3})\](.*?)\[/size\]"),
        font: re(r"\[font=([A-Za-z0-9 ,_-]{1,60})\](.*?)\[/font\]"),
        headings: re(r"\[h([1-6])\](.*?)\[/h([1-6])\](?:\r\n|\n)?"),
    }
});

pub fn bbcode2html(input: &str) -> String {
    let rules = &*RULES;
    let mut out = input.to_owned();

    for (regex, open, close) in &rules.simple {
        out = replace_loop(out, regex, |c| format!("{open}{}{close}", &c[1]));
    }
    out = replace_loop(out, &rules.img, |c| match safe_url(&c[1]) {
        Some(url) => format!("<img class=\"img-fluid\" src=\"{url}\" alt=\"\" />"),
        None => String::new(),
    });
    out = replace_loop(out, &rules.video, |c| {
        format!(
            "<iframe src=\"https://www.youtube.com/embed/{}\" width=\"640\" height=\"480\" frameborder=\"0\"></iframe>",
            &c[1]
        )
    });
    out = replace_loop(out, &rules.url, |c| match safe_url(&c[1]) {
        Some(url) => format!("<a href=\"{url}\">{url}</a>"),
        None => c[1].to_owned(),
    });
    out = replace_loop(out, &rules.url_named, |c| match safe_url(&c[1]) {
        Some(url) => format!("<a href=\"{url}\">{}</a>", &c[2]),
        None => c[2].to_owned(),
    });
    out = replace_loop(out, &rules.list, |c| format!("<ul>{}</ul>", c[1].trim()));
    out = replace_loop(out, &rules.list_ordered, |c| format!("<ol>{}</ol>", c[1].trim()));
    out = replace_loop(out, &rules.item, |c| format!("<li>{}</li>", &c[1]));
    out = replace_loop(out, &rules.color, |c| {
        format!("<span style=\"color: {};\">{}</span>", &c[1], &c[2])
    });
    out = replace_loop(out, &rules.size, |c| {
        format!("<span style=\"font-size: {}%;\">{}</span>", &c[1], &c[2])
    });
    out = replace_loop(out, &rules.font, |c| {
        format!("<span style=\"font-family: '{}';\">{}</span>", &c[1], &c[2])
    });
    out = replace_loop(out, &rules.headings, |c| {
        if c[1] == c[3] {
            format!("<h{0}>{1}</h{0}>", &c[1], &c[2])
        } else {
            c[2].to_owned()
        }
    });
    out
}

/// Équivalent du helper `bbcode()` : conversion puis `nl2br`.
pub fn render(input: &str) -> String {
    bbcode2html(input)
        .replace("\r\n", "\n")
        .replace('\n', "<br />\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_tags() {
        assert_eq!(bbcode2html("[b]gras[/b] et [i]ital[/i]"), "<b>gras</b> et <i>ital</i>");
        assert_eq!(bbcode2html("[b][u]x[/u][/b]"), "<b><u>x</u></b>");
        assert_eq!(bbcode2html("[h2]Titre[/h2]\nsuite"), "<h2>Titre</h2>suite");
    }

    #[test]
    fn lists_and_links() {
        assert_eq!(
            bbcode2html("[list][*]a[/*]\n[*]b[/*][/list]"),
            "<ul><li>a</li><li>b</li></ul>"
        );
        assert_eq!(
            bbcode2html("[url=https://neofr.ag]site[/url]"),
            "<a href=\"https://neofr.ag\">site</a>"
        );
    }

    #[test]
    fn dangerous_urls_are_dropped() {
        assert_eq!(bbcode2html("[url=javascript:alert(1)]clic[/url]"), "clic");
        assert_eq!(bbcode2html("[img]javascript:alert(1)[/img]"), "");
        assert_eq!(bbcode2html("[url]data:text/html,x[/url]"), "data:text/html,x");
    }

    #[test]
    fn newlines() {
        assert_eq!(render("a\r\nb"), "a<br />\nb");
    }
}
