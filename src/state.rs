use crate::{config::Config, core::mail::Mailer};
use sqlx::MySqlPool;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

#[derive(Clone)]
pub struct AppState {
    pub db: MySqlPool,
    pub config: Arc<Config>,
    /// `None` si l'envoi d'emails n'est pas configuré.
    pub mailer: Option<Arc<Mailer>>,
    /// Compteurs anti-abus en mémoire (voir `core::throttle`) : clé → (nombre, début de fenêtre).
    pub throttle: Arc<Mutex<HashMap<String, (u32, Instant)>>>,
}
