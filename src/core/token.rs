//! Jetons à usage unique envoyés par email (`nf_user_token`) : validation
//! d'inscription et réinitialisation de mot de passe.
//!
//! La table d'origine n'a que deux colonnes (`id` de 32 caractères, `user_id`) : ni
//! type, ni date d'expiration. Pour garder le schéma tout en étant plus sûr que
//! l'original :
//! - le jeton envoyé contient son type et sa date d'expiration ;
//! - la base ne stocke que l'empreinte SHA-256 (tronquée à 32 caractères) du jeton.
//!   Lire la base ne permet donc pas d'utiliser un jeton, et comme l'empreinte couvre
//!   le type et l'expiration, ils ne peuvent pas être modifiés.
//!
//! Conséquence : les jetons émis par le site PHP ne sont pas reconnus (il suffit de
//! refaire la demande).

use super::{ctx::Ctx, password};
use crate::error::AppError;
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    /// Validation de l'adresse email à l'inscription.
    Validation,
    /// Réinitialisation du mot de passe.
    Password,
}

impl Kind {
    fn tag(self) -> char {
        match self {
            Kind::Validation => 'v',
            Kind::Password => 'p',
        }
    }

    pub fn lifetime(self) -> Duration {
        match self {
            Kind::Validation => Duration::from_secs(7 * 24 * 3600),
            Kind::Password => Duration::from_secs(2 * 3600),
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Identifiant stocké en base pour un jeton.
fn db_id(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))[..32].to_owned()
}

/// Fabrique un jeton : type (1) + expiration en hexadécimal (8) + aléa (23).
fn generate(kind: Kind, now: u64) -> String {
    let expires = now.saturating_add(kind.lifetime().as_secs()).min(u32::MAX as u64);
    format!("{}{:08x}{}", kind.tag(), expires, &password::unique_id()[..23])
}

/// Vérifie la forme, le type et l'expiration ; renvoie l'identifiant à chercher en base.
fn check(token: &str, kind: Kind, now: u64) -> Option<String> {
    let well_formed = token.len() == 32
        && token.starts_with(kind.tag())
        && token.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    if !well_formed {
        return None;
    }
    let expires = u64::from_str_radix(&token[1..9], 16).ok()?;
    (expires > now).then(|| db_id(token))
}

/// Crée un jeton pour l'utilisateur et renvoie sa valeur (à mettre dans le lien).
pub async fn create(ctx: &Ctx, user_id: u32, kind: Kind) -> Result<String, AppError> {
    let token = generate(kind, now());
    sqlx::query("INSERT INTO nf_user_token (id, user_id) VALUES (?, ?)")
        .bind(db_id(&token))
        .bind(user_id)
        .execute(&ctx.state.db)
        .await?;
    Ok(token)
}

/// Utilisateur (non supprimé) désigné par un jeton valide, sans le consommer.
pub async fn user_of(ctx: &Ctx, token: &str, kind: Kind) -> Result<Option<u32>, AppError> {
    let Some(id) = check(token, kind, now()) else {
        return Ok(None);
    };
    Ok(sqlx::query_scalar(
        "SELECT t.user_id FROM nf_user_token t INNER JOIN nf_user u ON u.id = t.user_id AND u.deleted = '0' WHERE t.id = ?",
    )
    .bind(id)
    .fetch_optional(&ctx.state.db)
    .await?)
}

/// Invalide tous les jetons d'un utilisateur (après usage).
pub async fn revoke_all(ctx: &Ctx, user_id: u32) -> Result<(), AppError> {
    sqlx::query("DELETE FROM nf_user_token WHERE user_id = ?")
        .bind(user_id)
        .execute(&ctx.state.db)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let token = generate(Kind::Password, 1_000);
        assert_eq!(token.len(), 32);
        let id = check(&token, Kind::Password, 1_000).expect("valide");
        assert_eq!(id.len(), 32);
        assert_ne!(id, token, "la base ne doit pas contenir le jeton lui-même");
        assert_eq!(check(&token, Kind::Password, 1_000 + 7_199), Some(id));
    }

    #[test]
    fn expiry_and_kind_are_enforced() {
        let token = generate(Kind::Password, 1_000);
        assert_eq!(check(&token, Kind::Password, 1_000 + 7_200), None);
        assert_eq!(check(&token, Kind::Validation, 1_000), None);

        // Repousser l'expiration change l'empreinte : le jeton ne correspond plus à la base.
        let forged = format!("p{:08x}{}", u32::MAX, &token[9..]);
        assert_ne!(check(&forged, Kind::Password, 1_000), check(&token, Kind::Password, 1_000));
        // Changer le type aussi.
        let retagged = format!("v{}", &token[1..]);
        assert_ne!(check(&retagged, Kind::Validation, 1_000), check(&token, Kind::Password, 1_000));
    }

    #[test]
    fn malformed_tokens() {
        for bad in ["", "p", "p0000zzzz", &"p".repeat(31), &"p".repeat(33), "P00000000aaaaaaaaaaaaaaaaaaaaaaa", "pé000000aaaaaaaaaaaaaaaaaaaaaaa"] {
            assert_eq!(check(bad, Kind::Password, 0), None, "{bad}");
        }
    }
}
