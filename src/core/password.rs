//! Mots de passe compatibles NeoFrag (phpass « portable », préfixes `$P$` / `$H$`).
//!
//! NeoFrag hache `sel_global + mot_de_passe + sel_utilisateur` avec phpass. Les très
//! anciens comptes (sel utilisateur vide, dont le compte `admin` d'installation) sont
//! hachés sans aucun sel : on les accepte et l'appelant les re-hache à la connexion.
//!
//! phpass repose sur MD5 itéré : c'est faible selon les standards actuels, mais la
//! colonne `nf_user.password` fait 34 caractères. Voir ROADMAP.md (migration Argon2).

use md5::{Digest, Md5};
use rand::RngCore;

const ITOA64: &[u8; 64] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Nombre d'itérations (log2) utilisé par NeoFrag pour les nouveaux hachages.
const ITERATION_COUNT_LOG2: usize = 8 + 5;

fn encode64(input: &[u8], count: usize) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < count {
        let mut value = input[i] as u32;
        i += 1;
        out.push(ITOA64[(value & 0x3f) as usize] as char);
        if i < count {
            value |= (input[i] as u32) << 8;
        }
        out.push(ITOA64[((value >> 6) & 0x3f) as usize] as char);
        if i >= count {
            break;
        }
        i += 1;
        if i < count {
            value |= (input[i] as u32) << 16;
        }
        out.push(ITOA64[((value >> 12) & 0x3f) as usize] as char);
        if i >= count {
            break;
        }
        i += 1;
        out.push(ITOA64[((value >> 18) & 0x3f) as usize] as char);
    }
    out
}

/// `crypt_private` de phpass. Renvoie `None` si `setting` n'est pas un hachage phpass.
fn crypt_private(password: &[u8], setting: &str) -> Option<String> {
    let bytes = setting.as_bytes();
    if bytes.len() < 12 || !(setting.starts_with("$P$") || setting.starts_with("$H$")) {
        return None;
    }
    let count_log2 = ITOA64.iter().position(|&c| c == bytes[3])?;
    if !(7..=30).contains(&count_log2) {
        return None;
    }
    let salt = &bytes[4..12];

    let mut hash = Md5::new().chain_update(salt).chain_update(password).finalize();
    for _ in 0..(1u64 << count_log2) {
        hash = Md5::new().chain_update(hash).chain_update(password).finalize();
    }

    // Les 12 premiers octets sont de l'ASCII (vérifié par `position` et `get`), mais
    // `setting` vient de la base : on ne coupe pas sans vérifier.
    let prefix = setting.get(..12)?;
    Some(format!("{prefix}{}", encode64(&hash, 16)))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub struct Verified {
    /// Le hachage stocké date d'avant les sels : à régénérer avec [`hash`].
    pub needs_rehash: bool,
}

/// Vérifie un mot de passe contre `nf_user.password` / `nf_user.salt`.
pub fn verify(global_salt: &str, password: &str, stored_hash: &str, user_salt: &str) -> Option<Verified> {
    let candidate = if user_salt.is_empty() {
        password.to_owned()
    } else {
        format!("{global_salt}{password}{user_salt}")
    };
    let computed = crypt_private(candidate.as_bytes(), stored_hash)?;
    constant_time_eq(computed.as_bytes(), stored_hash.as_bytes()).then_some(Verified {
        needs_rehash: user_salt.is_empty(),
    })
}

/// Identifiant aléatoire de 32 caractères `[a-z0-9]` (sessions, sels, jetons),
/// même forme que le `unique_id()` de NeoFrag.
pub fn unique_id() -> String {
    const CHARS: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    // 252 = 7 × 36 : on rejette au-delà pour éviter le biais du modulo.
    let mut out = String::with_capacity(32);
    let mut pool = bytes.to_vec();
    while out.len() < 32 {
        if pool.is_empty() {
            let mut more = [0u8; 16];
            rand::rngs::OsRng.fill_bytes(&mut more);
            pool.extend_from_slice(&more);
        }
        let b = pool.pop().unwrap_or_default();
        if b < 252 {
            out.push(CHARS[(b % 36) as usize] as char);
        }
    }
    out
}

/// Hache un nouveau mot de passe. Renvoie `(hash, sel_utilisateur)`.
pub fn hash(global_salt: &str, password: &str) -> (String, String) {
    let user_salt = unique_id();
    let mut random = [0u8; 6];
    rand::rngs::OsRng.fill_bytes(&mut random);
    let setting = format!(
        "$P${}{}",
        ITOA64[ITERATION_COUNT_LOG2] as char,
        encode64(&random, 6)
    );
    let candidate = format!("{global_salt}{password}{user_salt}");
    let hash = crypt_private(candidate.as_bytes(), &setting).unwrap_or_else(|| "*".to_owned());
    (hash, user_salt)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hachage du compte `admin` / `admin123` livré dans DATABASE.sql.
    const INSTALL_ADMIN: &str = "$H$92EwygSmbdXunbIvoo/V91MWcnHqzX/";

    #[test]
    fn install_admin_password() {
        let ok = verify("peu-importe", "admin123", INSTALL_ADMIN, "").expect("doit être accepté");
        assert!(ok.needs_rehash);
        assert!(verify("peu-importe", "admin124", INSTALL_ADMIN, "").is_none());
    }

    #[test]
    fn roundtrip_with_salts() {
        let (stored, salt) = hash("sel-global", "s3cret é");
        assert_eq!(stored.len(), 34);
        assert_eq!(salt.len(), 32);
        let ok = verify("sel-global", "s3cret é", &stored, &salt).unwrap();
        assert!(!ok.needs_rehash);
        assert!(verify("autre-sel", "s3cret é", &stored, &salt).is_none());
        assert!(verify("sel-global", "s3cret", &stored, &salt).is_none());
    }

    #[test]
    fn rejects_non_phpass() {
        assert!(verify("", "x", "", "").is_none());
        assert!(verify("", "x", "*", "").is_none());
        assert!(verify("", "x", "$2y$10$abcdefghijklmnopqrstuv", "").is_none());
        assert!(verify("", "x", "$P$éééééééééééé", "").is_none());
    }

    #[test]
    fn unique_ids() {
        let a = unique_id();
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
        assert_ne!(a, unique_id());
    }
}
