//! Champs de formulaire Bootstrap partagés par le site et l'administration.
//!
//! Les valeurs reçues sont des textes bruts : elles sont encodées ici. Les libellés
//! et les aides sont du HTML fourni par le code (jamais par l'utilisateur).

use super::{ctx::Ctx, text};

pub fn csrf_field(ctx: &Ctx) -> String {
    format!("<input type=\"hidden\" name=\"csrf\" value=\"{}\" />", ctx.csrf_token())
}

fn help_html(help: &str) -> String {
    if help.is_empty() {
        String::new()
    } else {
        format!("<small class=\"form-text text-muted\">{help}</small>")
    }
}

#[allow(clippy::too_many_arguments)]
fn typed_input(kind: &str, label: &str, name: &str, value: &str, max: usize, required: bool, help: &str, extra: &str) -> String {
    format!(
        "<div class=\"form-group\"><label for=\"f-{name}\"><b>{label}</b></label>\
         <input type=\"{kind}\" class=\"form-control\" id=\"f-{name}\" name=\"{name}\" value=\"{}\" maxlength=\"{max}\"{}{extra} />{}</div>",
        text::encode(value),
        if required { " required" } else { "" },
        help_html(help),
    )
}

pub fn input(label: &str, name: &str, value: &str, max: usize, required: bool, help: &str) -> String {
    typed_input("text", label, name, value, max, required, help, "")
}

pub fn email(label: &str, name: &str, value: &str, required: bool) -> String {
    typed_input("email", label, name, value, 100, required, "", " autocomplete=\"email\"")
}

pub fn date(label: &str, name: &str, value: &str) -> String {
    typed_input("date", label, name, value, 10, false, "", "")
}

/// Champ mot de passe : jamais pré-rempli. `autocomplete` : `current-password` ou `new-password`.
pub fn password(label: &str, name: &str, autocomplete: &str, required: bool, help: &str) -> String {
    format!(
        "<div class=\"form-group\"><label for=\"f-{name}\"><b>{label}</b></label>\
         <input type=\"password\" class=\"form-control\" id=\"f-{name}\" name=\"{name}\" maxlength=\"200\" autocomplete=\"{autocomplete}\"{} />{}</div>",
        if required { " required" } else { "" },
        help_html(help),
    )
}

pub fn textarea(label: &str, name: &str, value: &str, rows: u8, required: bool) -> String {
    format!(
        "<div class=\"form-group\"><label for=\"f-{name}\"><b>{label}</b> <small class=\"text-muted\">BBCode accepté</small></label>\
         <textarea class=\"form-control\" id=\"f-{name}\" name=\"{name}\" rows=\"{rows}\"{}>{}</textarea></div>",
        if required { " required" } else { "" },
        text::encode(value),
    )
}

pub fn checkbox(label: &str, name: &str, checked: bool) -> String {
    format!(
        "<div class=\"form-group form-check\"><input type=\"checkbox\" class=\"form-check-input\" id=\"f-{name}\" name=\"{name}\" value=\"on\"{} />\
         <label class=\"form-check-label\" for=\"f-{name}\">{label}</label></div>",
        if checked { " checked" } else { "" },
    )
}

/// Liste déroulante. `options` : couples (valeur, libellé), bruts.
pub fn select<'a>(label: &str, name: &str, selected: &str, options: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let options: String = options
        .into_iter()
        .map(|(value, title)| {
            format!(
                "<option value=\"{}\"{}>{}</option>",
                text::encode(value),
                if value == selected { " selected" } else { "" },
                text::encode(title)
            )
        })
        .collect();
    format!(
        "<div class=\"form-group\"><label for=\"f-{name}\"><b>{label}</b></label>\
         <select class=\"form-control\" id=\"f-{name}\" name=\"{name}\">{options}</select></div>"
    )
}

/// Champ d'envoi d'image, avec aperçu de l'image actuelle (`current` : URL déjà construite).
pub fn image(label: &str, name: &str, current: Option<&str>, help: &str) -> String {
    let preview = match current {
        Some(url) => format!(
            "<div class=\"mb-2\"><img src=\"{}\" alt=\"\" style=\"max-height: 120px; max-width: 100%;\" /></div>",
            text::encode(url)
        ),
        None => String::new(),
    };
    format!(
        "<div class=\"form-group\"><label for=\"f-{name}\"><b>{label}</b></label>{preview}\
         <input type=\"file\" class=\"form-control-file\" id=\"f-{name}\" name=\"{name}\" accept=\"image/png,image/jpeg,image/gif,image/webp\" />{}</div>",
        help_html(help),
    )
}

pub fn alert(kind: &str, message: Option<&str>) -> String {
    match message {
        Some(message) => format!("<div class=\"alert alert-{kind}\">{message}</div>"),
        None => String::new(),
    }
}

/// Longueur en caractères une fois encodé, telle que stockée en base (colonnes `varchar`).
pub fn fits(value: &str, max: usize) -> bool {
    text::encode(value).chars().count() <= max
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_encoded() {
        let html = input("Titre", "title", "\"><script>", 100, true, "");
        assert!(!html.contains("<script>"));
        assert!(html.contains("&quot;&gt;&lt;script&gt;"));
        let html = select("Pays", "country", "fr", [("fr", "France"), ("x", "<b>")]);
        assert!(html.contains("<option value=\"fr\" selected>France</option>"));
        assert!(html.contains("&lt;b&gt;"));
    }

    #[test]
    fn encoded_length() {
        assert!(fits(&"a".repeat(100), 100));
        assert!(!fits(&"&".repeat(30), 100));
    }
}
