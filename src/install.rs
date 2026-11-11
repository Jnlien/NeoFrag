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
use std::{io::Write, str::FromStr};

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

pub async fn run(config: &Config) -> anyhow::Result<()> {
    if config.security.password_salt.starts_with("REMPLACEZ") || config.security.secret_key.starts_with("REMPLACEZ") {
        bail!("renseignez d'abord security.password_salt et security.secret_key dans neofrag.toml (la commande `neofrag secrets` en génère)");
    }

    // Connexion dédiée : le script règle des variables de session (fuseau, clés étrangères)
    // qui ne doivent pas rester sur une connexion réutilisée.
    let mut db = MySqlConnectOptions::from_str(&config.database.url)?
        .connect()
        .await
        .context("connexion à la base impossible")?;

    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name LIKE 'nf\\_%'")
        .fetch_one(&mut db)
        .await?;
    if existing > 0 {
        bail!("la base contient déjà {existing} tables NeoFrag : installation annulée, rien n'a été modifié (lancez simplement le serveur pour reprendre ce site)");
    }

    println!("Installation de NeoFrag dans une base vide.\n");
    let site = ask("NEOFRAG_SITE_NAME", "Nom du site", false)?;
    let username = ask("NEOFRAG_ADMIN_USER", "Identifiant de l'administrateur", false)?;
    let email = ask("NEOFRAG_ADMIN_EMAIL", "Adresse email de l'administrateur", false)?;
    let password = ask("NEOFRAG_ADMIN_PASSWORD", "Mot de passe (8 caractères minimum)", true)?;
    if std::env::var("NEOFRAG_ADMIN_PASSWORD").is_err() {
        let confirm = ask("", "Confirmation du mot de passe", true)?;
        if confirm != password {
            bail!("les deux mots de passe ne correspondent pas");
        }
    }
    if site.is_empty() || site.chars().count() > 100 {
        bail!("le nom du site est obligatoire (100 caractères maximum)");
    }
    account::validate_username(&username).map_err(|e| anyhow::anyhow!("identifiant : {e}"))?;
    account::validate_email(&email).map_err(|e| anyhow::anyhow!("email : {e}"))?;
    account::validate_new_password(&password, &password).map_err(|e| anyhow::anyhow!("mot de passe : {e}"))?;

    sqlx::raw_sql(SCHEMA).execute(&mut db).await.context("création du schéma")?;

    let (hash, salt) = password::hash(&config.security.password_salt, &password);
    sqlx::query("UPDATE nf_user SET username = ?, password = ?, salt = ?, email = ?, registration_date = NOW() WHERE id = 1")
        .bind(text::encode(&username))
        .bind(hash)
        .bind(salt)
        .bind(&email)
        .execute(&mut db)
        .await?;
    sqlx::query("UPDATE nf_settings SET value = ? WHERE name = 'nf_name'")
        .bind(text::encode(&site))
        .execute(&mut db)
        .await?;
    sqlx::query("UPDATE nf_settings SET value = ? WHERE name = 'nf_contact'")
        .bind(&email)
        .execute(&mut db)
        .await?;
    db.close().await?;

    std::fs::create_dir_all(&config.site.upload_dir)
        .with_context(|| format!("création du dossier {}", config.site.upload_dir.display()))?;

    println!("\nInstallation terminée. Lancez le serveur (`neofrag`), puis connectez-vous avec « {username} ».");
    Ok(())
}
