//! Utilisateur connecté et helpers d'affichage (`nf_user`, `nf_user_profile`).

use super::text;

#[derive(Debug, Clone)]
pub struct User {
    pub id: u32,
    pub username: String,
    pub admin: bool,
    /// Chemin du fichier d'avatar (`nf_file.path`), s'il y en a un.
    pub avatar: Option<String>,
    pub sex: Option<String>,
}

/// Lien vers un profil, équivalent de `$this->user->link()`.
pub fn link(base: &str, user_id: u32, username: &str) -> String {
    format!(
        "<a class=\"user-profile-link\" href=\"{}\">{}</a>",
        text::url(base, &format!("user/{}/{}", user_id, text::url_title(username))),
        text::clean(username)
    )
}

/// URL de l'image d'avatar (image par défaut selon le sexe renseigné).
pub fn avatar_url(base: &str, avatar: Option<&str>, sex: Option<&str>) -> String {
    match avatar {
        Some(path) if !path.is_empty() => text::url(base, path),
        _ if sex == Some("female") => text::url(base, "images/default_avatar_female.jpg"),
        _ => text::url(base, "images/default_avatar_male.jpg"),
    }
}

/// Bloc avatar rond, équivalent de `$user->avatar()`.
pub fn avatar_html(base: &str, avatar: Option<&str>, sex: Option<&str>, online: bool, extra_class: &str) -> String {
    format!(
        "<div class=\"avatar {}{}\"><div class=\"inner-ring\"></div><figure><img src=\"{}\" alt=\"\" /></figure></div>",
        if online { "online" } else { "offline" },
        if extra_class.is_empty() { String::new() } else { format!(" {}", text::css_classes(extra_class)) },
        text::encode(&avatar_url(base, avatar, sex)),
    )
}
