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
    #[serde(default)]
    pub email: Email,
}

/// Envoi d'emails par SMTP (remplace `config/email.php`). Sans `smtp_host` ou sans
/// `server.public_url`, les fonctions qui envoient des emails sont désactivées.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Email {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    pub smtp_password: String,
    /// `starttls` (port 587), `tls` (port 465) ou `none` (serveur local uniquement).
    pub smtp_security: String,
}

impl Default for Email {
    fn default() -> Self {
        Self {
            smtp_host: String::new(),
            smtp_port: 587,
            smtp_username: String::new(),
            smtp_password: String::new(),
            smtp_security: "starttls".to_owned(),
        }
    }
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
    /// Adresse publique du site, sans chemin (`https://exemple.fr`). Sert à écrire les
    /// liens dans les emails : elle n'est jamais déduite de la requête, qu'un
    /// visiteur peut falsifier.
    pub public_url: String,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8080".to_owned(),
            base_url: String::new(),
            https: false,
            public_url: String::new(),
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

/// Variable d'environnement non vide.
fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

fn random_hex() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

impl Config {
    /// Premier lancement : écrit un `neofrag.toml` prêt à l'emploi, avec des secrets
    /// tirés au hasard. L'adresse de la base vient de `NEOFRAG_DATABASE_URL` si elle est
    /// définie ; sinon elle reste à renseigner dans le fichier.
    fn create_default(path: &Path) -> anyhow::Result<()> {
        let database = env("NEOFRAG_DATABASE_URL").unwrap_or_else(|| "mysql://utilisateur:motdepasse@localhost:3306/neofrag".to_owned());
        let content = include_str!("../neofrag.example.toml")
            .replace("bind = \"127.0.0.1:8080\"", "bind = \"0.0.0.0:8080\"")
            .replace("mysql://utilisateur:motdepasse@localhost:3306/neofrag", &database.replace('\\', "\\\\").replace('"', "\\\""))
            .replace("password_salt = \"REMPLACEZ-MOI\"", &format!("password_salt = \"{}\"", random_hex()))
            .replace(
                "secret_key = \"REMPLACEZ-MOI-PAR-AU-MOINS-32-CARACTERES-ALEATOIRES\"",
                &format!("secret_key = \"{}\"", random_hex()),
            );
        // Lisible par son seul propriétaire : le fichier contient les clés et le mot de passe de la base.
        {
            use std::{io::Write, os::unix::fs::OpenOptionsExt};
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
                .and_then(|mut file| file.write_all(content.as_bytes()))
                .map_err(|e| anyhow::anyhow!("impossible de créer {} : {e}", path.display()))?;
        }
        println!("Fichier de configuration créé : {}", path.display());
        Ok(())
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            Self::create_default(path)?;
        }
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("impossible de lire {} : {e}", path.display()))?;
        let mut config: Config = toml::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("{} invalide : {e}", path.display()))?;

        // L'environnement prime sur le fichier (conteneurs, panels d'hébergement).
        if let Some(url) = env("NEOFRAG_DATABASE_URL") {
            config.database.url = url;
        }
        if let Some(bind) = env("NEOFRAG_BIND") {
            config.server.bind = bind;
        } else if let Some(port) = env("NEOFRAG_PORT").or_else(|| env("SERVER_PORT")) {
            // `SERVER_PORT` : port attribué par un panel (Pterodactyl…).
            config.server.bind = format!("0.0.0.0:{port}");
        }
        if let Some(url) = env("NEOFRAG_PUBLIC_URL") {
            config.server.https = url.starts_with("https://");
            config.server.public_url = url;
        }
        if config.database.url.contains("utilisateur:motdepasse@") {
            anyhow::bail!(
                "indiquez l'adresse de la base MySQL dans {} (ligne « url = » de [database]), puis relancez",
                path.display()
            );
        }
        if config.security.password_salt.starts_with("REMPLACEZ") || config.security.secret_key.starts_with("REMPLACEZ") {
            anyhow::bail!("renseignez security.password_salt et security.secret_key dans {} (`neofrag secrets` en génère)", path.display());
        }
        // Dossiers relatifs introuvables depuis le dossier courant : on les cherche à
        // côté de l'exécutable (lancement par un service ou un panel).
        if config.site.assets_dir.is_relative() && !config.site.assets_dir.exists() {
            if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
                if dir.join(&config.site.assets_dir).exists() {
                    if config.site.upload_dir.is_relative() {
                        config.site.upload_dir = dir.join(&config.site.upload_dir);
                    }
                    config.site.assets_dir = dir.join(&config.site.assets_dir);
                }
            }
        }

        config.server.base_url = config.server.base_url.trim_end_matches('/').to_owned();
        if !config.server.base_url.is_empty() && !config.server.base_url.starts_with('/') {
            anyhow::bail!("server.base_url doit commencer par '/' (ou être vide)");
        }
        config.server.public_url = config.server.public_url.trim_end_matches('/').to_owned();
        let public_url = &config.server.public_url;
        if !public_url.is_empty() && !(public_url.starts_with("https://") || public_url.starts_with("http://")) {
            anyhow::bail!("server.public_url doit commencer par https:// (ou http://)");
        }
        if !matches!(config.email.smtp_security.as_str(), "starttls" | "tls" | "none") {
            anyhow::bail!("email.smtp_security doit valoir \"starttls\", \"tls\" ou \"none\"");
        }
        if config.security.secret_key.len() < 32 {
            anyhow::bail!("security.secret_key doit faire au moins 32 caractères");
        }
        Ok(config)
    }
}
