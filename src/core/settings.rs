//! Réglages du site stockés dans `nf_settings` (équivalent de `$this->config`).

use sqlx::{MySqlPool, Row};
use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct Settings(HashMap<String, String>);

impl Settings {
    /// Charge les réglages globaux, surchargés par ceux propres à la langue.
    pub async fn load(db: &MySqlPool, lang: &str) -> sqlx::Result<Self> {
        let rows = sqlx::query(
            "SELECT name, IFNULL(value, '') AS value FROM nf_settings \
             WHERE site = '' AND lang IN ('', ?) ORDER BY lang",
        )
        .bind(lang)
        .fetch_all(db)
        .await?;

        let mut map = HashMap::with_capacity(rows.len());
        for row in rows {
            map.insert(row.try_get::<String, _>("name")?, row.try_get::<String, _>("value")?);
        }
        Ok(Self(map))
    }

    pub fn get(&self, name: &str) -> &str {
        self.0.get(name).map(String::as_str).unwrap_or("")
    }

    pub fn int(&self, name: &str) -> i64 {
        self.get(name).trim().parse().unwrap_or(0)
    }

    pub fn bool(&self, name: &str) -> bool {
        !matches!(self.get(name).trim(), "" | "0" | "false")
    }

    /// Écrit un réglage global en conservant son type déclaré s'il existe déjà.
    pub async fn set(db: &MySqlPool, name: &str, value: &str, kind: &str) -> sqlx::Result<()> {
        sqlx::query(
            "INSERT INTO nf_settings (name, site, lang, value, type) VALUES (?, '', '', ?, ?) \
             ON DUPLICATE KEY UPDATE value = VALUES(value)",
        )
        .bind(name)
        .bind(value)
        .bind(kind)
        .execute(db)
        .await?;
        Ok(())
    }
}
