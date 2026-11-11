//! Configuration du serveur (fichier `neofrag.toml`).
//!
//! Remplace les fichiers `config/*.php` de NeoFrag. Le reste des réglages (nom du
//! site, thème, page d'accueil…) reste en base dans `nf_settings`, comme avant.

use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: Server,
    pub database: Database,
    pub security: Security,
    #[serde(default)]
    pub site: Site,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Server {
    /// Adresse d'écoute.
    pub bind: String,
    /// Préfixe d'URL si le site n'est pas à la racine du domaine (ex. `/site`).
    pub base_url: String,
    /// À activer derrière un reverse proxy HTTPS : cookies `Secure`.
    pub https: bool,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8080".to_owned(),
            base_url: String::new(),
            https: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Database {
    /// `mysql://utilisateur:motdepasse@hote:3306/base`
    pub url: String,
    #[serde(default = "default_pool")]
    pub max_connections: u32,
}

fn default_pool() -> u32 {
    10
}

#[derive(Debug, Clone, Deserialize)]
pub struct Security {
    /// Valeur de `$password['salt']` dans `config/password.php` du site d'origine.
    /// Indispensable pour que les comptes existants puissent se connecter.
    pub password_salt: String,
    /// Clé secrète du serveur (jetons anti-CSRF). 32 caractères aléatoires minimum.
    pub secret_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Site {
    /// Langue des contenus (colonnes `lang` des tables `*_lang`).
    pub lang: String,
    /// Dossier des fichiers statiques (css, js, thèmes…).
    pub assets_dir: PathBuf,
    /// Dossier des fichiers envoyés (les chemins `upload/...` de `nf_file`).
    pub upload_dir: PathBuf,
    /// Taille maximale d'un fichier envoyé, en mégaoctets.
    pub max_upload_mb: usize,
}

impl Default for Site {
    fn default() -> Self {
        Self {
            lang: "fr".to_owned(),
            assets_dir: PathBuf::from("assets"),
            upload_dir: PathBuf::from("assets/upload"),
            max_upload_mb: 5,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("impossible de lire {} : {e}", path.display()))?;
        let mut config: Config = toml::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("{} invalide : {e}", path.display()))?;

        config.server.base_url = config.server.base_url.trim_end_matches('/').to_owned();
        if !config.server.base_url.is_empty() && !config.server.base_url.starts_with('/') {
            anyhow::bail!("server.base_url doit commencer par '/' (ou être vide)");
        }
        if config.security.secret_key.len() < 32 {
            anyhow::bail!("security.secret_key doit faire au moins 32 caractères");
        }
        Ok(config)
    }
}
