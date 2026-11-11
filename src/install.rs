//! Installation neuve en ligne de commande : `neofrag install [neofrag.toml]`.
//!
//! Remplace l'assistant web de l'original (`install/`) : crée le schéma de NeoFrag
//! Alpha 0.2.3 dans une base **vide**, puis le compte administrateur. Rien n'est fait si
//! la base contient déjà des tables NeoFrag — pour reprendre un site existant, il suffit
//! de lancer le serveur sur sa base.

use crate::{
    config::Config,
    core::{password, text},
    modules::account,
};
use anyhow::{bail, Context};
use rand::RngCore;
use sqlx::{mysql::MySqlConnectOptions, ConnectOptions, Connection};
use std::{io::Write, path::Path, str::FromStr};

/// Schéma et données initiales de l'original, embarqués dans le binaire.
const SCHEMA: &str = include_str!("../legacy/DATABASE.sql");

/// `neofrag secrets` : deux valeurs aléatoires à recopier dans `neofrag.toml`.
pub fn print_secrets() {
    let random = || {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        hex::encode(bytes)
    };
    println!("[security]\npassword_salt = \"{}\"\nsecret_key = \"{}\"", random(), random());
}

/// Valeur d'une variable d'environnement, sinon question posée dans le terminal.
fn ask(variable: &str, question: &str, secret: bool) -> anyhow::Result<String> {
    if let Ok(value) = std::env::var(variable) {
        return Ok(value.trim().to_owned());
    }
    if secret {
        return Ok(rpassword::prompt_password(format!("{question} : "))?);
    }
    print!("{question} : ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_owned())
}

/// Connexion dédiée : le script règle des variables de session (fuseau, clés étrangères)
/// qui ne doivent pas rester sur une connexion réutilisée.
async fn connect(config: &Config) -> anyhow::Result<sqlx::MySqlConnection> {
    MySqlConnectOptions::from_str(&config.database.url)?
        .connect()
        .await
        .context("connexion à la base impossible")
}

async fn neofrag_tables(db: &mut sqlx::MySqlConnection) -> anyhow::Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name LIKE 'nf\\_%'")
            .fetch_one(db)
            .await?,
    )
}

struct Answers {
    site: String,
    username: String,
    email: String,
    password: String,
}

impl Answers {
    fn validate(&self) -> anyhow::Result<()> {
        if self.site.is_empty() || self.site.chars().count() > 100 {
            bail!("le nom du site est obligatoire (100 caractères maximum)");
        }
        account::validate_username(&self.username).map_err(|e| anyhow::anyhow!("identifiant : {e}"))?;
        account::validate_email(&self.email).map_err(|e| anyhow::anyhow!("email : {e}"))?;
        account::validate_new_password(&self.password, &self.password).map_err(|e| anyhow::anyhow!("mot de passe : {e}"))?;
        Ok(())
    }
}

/// Crée le schéma et le compte administrateur dans une base vide.
async fn create(config: &Config, mut db: sqlx::MySqlConnection, answers: &Answers) -> anyhow::Result<()> {
    sqlx::raw_sql(SCHEMA).execute(&mut db).await.context("création du schéma")?;

    let (hash, salt) = password::hash(&config.security.password_salt, &answers.password);
    sqlx::query("UPDATE nf_user SET username = ?, password = ?, salt = ?, email = ?, registration_date = NOW() WHERE id = 1")
        .bind(text::encode(&answers.username))
        .bind(hash)
        .bind(salt)
        .bind(&answers.email)
        .execute(&mut db)
        .await?;
    sqlx::query("UPDATE nf_settings SET value = ? WHERE name = 'nf_name'")
        .bind(text::encode(&answers.site))
        .execute(&mut db)
        .await?;
    sqlx::query("UPDATE nf_settings SET value = ? WHERE name = 'nf_contact'")
        .bind(&answers.email)
        .execute(&mut db)
        .await?;
    db.close().await?;

    std::fs::create_dir_all(&config.site.upload_dir)
        .with_context(|| format!("création du dossier {}", config.site.upload_dir.display()))?;
    Ok(())
}

/// `neofrag install` : installation guidée dans le terminal.
pub async fn run(config: &Config) -> anyhow::Result<()> {
    let mut db = connect(config).await?;
    let existing = neofrag_tables(&mut db).await?;
    if existing > 0 {
        bail!("la base contient déjà {existing} tables NeoFrag : installation annulée, rien n'a été modifié (lancez simplement le serveur pour reprendre ce site)");
    }

    println!("Installation de NeoFrag dans une base vide.\n");
    let answers = Answers {
        site: ask("NEOFRAG_SITE_NAME", "Nom du site", false)?,
        username: ask("NEOFRAG_ADMIN_USER", "Identifiant de l'administrateur", false)?,
        email: ask("NEOFRAG_ADMIN_EMAIL", "Adresse email de l'administrateur", false)?,
        password: ask("NEOFRAG_ADMIN_PASSWORD", "Mot de passe (8 caractères minimum)", true)?,
    };
    if std::env::var("NEOFRAG_ADMIN_PASSWORD").is_err() {
        let confirm = ask("", "Confirmation du mot de passe", true)?;
        if confirm != answers.password {
            bail!("les deux mots de passe ne correspondent pas");
        }
    }
    answers.validate()?;
    create(config, db, &answers).await?;
    println!("\nInstallation terminée. Lancez le serveur (`neofrag`), puis connectez-vous avec « {} ».", answers.username);
    Ok(())
}

/// Premier démarrage du serveur sur une base vide : installation automatique, sans
/// question. Les réponses viennent de l'environnement (`NEOFRAG_SITE_NAME`,
/// `NEOFRAG_ADMIN_USER`, `NEOFRAG_ADMIN_EMAIL`, `NEOFRAG_ADMIN_PASSWORD`) ; à défaut, le
/// compte « admin » reçoit un mot de passe aléatoire, affiché une fois et écrit dans
/// `identifiants-admin.txt` à côté du fichier de configuration.
/// Ne fait rien si la base contient déjà des tables NeoFrag.
pub async fn auto(config: &Config, config_path: &Path) -> anyhow::Result<()> {
    let mut db = connect(config).await?;
    if neofrag_tables(&mut db).await? > 0 {
        return Ok(());
    }
    let env = |name: &str| std::env::var(name).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    let generated = env("NEOFRAG_ADMIN_PASSWORD").is_none();
    let answers = Answers {
        site: env("NEOFRAG_SITE_NAME").unwrap_or_else(|| "Mon site NeoFrag".to_owned()),
        username: env("NEOFRAG_ADMIN_USER").unwrap_or_else(|| "admin".to_owned()),
        email: env("NEOFRAG_ADMIN_EMAIL").unwrap_or_else(|| "admin@example.com".to_owned()),
        password: env("NEOFRAG_ADMIN_PASSWORD").unwrap_or_else(|| {
            let mut bytes = [0u8; 9];
            rand::rngs::OsRng.fill_bytes(&mut bytes);
            hex::encode(bytes)
        }),
    };
    answers.validate()?;
    create(config, db, &answers).await?;

    println!("\n==================== NeoFrag : première installation ====================");
    println!("  Base de données créée. Compte administrateur :");
    println!("    identifiant  : {}", answers.username);
    if generated {
        println!("    mot de passe : {}", answers.password);
        let note = config_path.with_file_name("identifiants-admin.txt");
        let body = format!(
            "NeoFrag - compte administrateur créé à l'installation\n\nidentifiant  : {}\nmot de passe : {}\n\nChangez ce mot de passe après la première connexion (Gérer mon compte), puis supprimez ce fichier.\n",
            answers.username, answers.password
        );
        match write_private(&note, &body) {
            Ok(()) => println!("  Ces identifiants sont aussi dans {} : supprimez ce fichier ensuite.", note.display()),
            Err(e) => println!("  (impossible d'écrire {} : {e} - notez le mot de passe maintenant)", note.display()),
        }
        println!("  Changez le mot de passe après la première connexion.");
    } else {
        println!("    mot de passe : celui de NEOFRAG_ADMIN_PASSWORD");
    }
    println!("==========================================================================\n");
    Ok(())
}

/// Écrit un fichier lisible par son seul propriétaire (il contient un mot de passe).
fn write_private(path: &Path, content: &str) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    file.write_all(content.as_bytes())
}
