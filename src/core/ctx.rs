//! Contexte d'une requête : réglages, session, utilisateur, permissions.
//!
//! Joue le rôle du singleton `NeoFrag()` de l'original, mais construit une fois par
//! requête par le middleware [`super::session::middleware`] et partagé en lecture seule.

use super::{access::Access, settings::Settings, text, user::User};
use crate::state::AppState;
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub struct Ctx {
    pub state: AppState,
    pub settings: Settings,
    /// Identifiant de session (`nf_session.id`), vide pour les robots d'indexation.
    pub session_id: String,
    pub user: Option<User>,
    pub access: Access,
    /// Modules désactivés dans l'administration (`nf_addon`).
    pub disabled: Vec<String>,
    /// Chemin demandé, sans préfixe ni barre initiale (`news/3/titre`, vide pour l'accueil).
    pub path: String,
    pub ip: String,
    pub user_agent: String,
}

impl Ctx {
    pub fn base(&self) -> &str {
        &self.state.config.server.base_url
    }

    pub fn lang(&self) -> &str {
        &self.state.config.site.lang
    }

    /// Le module est-il activé ? (Un module absent de `nf_addon` est considéré actif.)
    pub fn enabled(&self, module: &str) -> bool {
        !self.disabled.iter().any(|name| name == module)
    }

    /// URL interne, équivalent du helper `url()`.
    /// Le widget est-il activé (lui-même, et le module du même nom s'il existe) ?
    pub fn widget_enabled(&self, widget: &str) -> bool {
        self.enabled(widget) && !self.disabled.iter().any(|name| name.strip_prefix("widget:") == Some(widget))
    }

    pub fn url(&self, path: &str) -> String {
        text::url(self.base(), path)
    }

    /// Jeton anti-CSRF lié à la session : HMAC(clé du serveur, identifiant de session).
    pub fn csrf_token(&self) -> String {
        csrf_for(&self.state.config.security.secret_key, &self.session_id)
    }

    pub fn csrf_ok(&self, token: &str) -> bool {
        if self.session_id.is_empty() {
            return false;
        }
        let Ok(given) = hex::decode(token) else {
            return false;
        };
        let mut mac = Hmac::<Sha256>::new_from_slice(self.state.config.security.secret_key.as_bytes())
            .expect("HMAC accepte toute taille de clé");
        mac.update(self.session_id.as_bytes());
        mac.verify_slice(&given).is_ok()
    }
}

fn csrf_for(secret: &str, session_id: &str) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepte toute taille de clé");
    mac.update(session_id.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}
