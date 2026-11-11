use crate::config::Config;
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
    /// Compteurs anti-abus en mémoire (voir `core::throttle`) : clé → (nombre, début de fenêtre).
    pub throttle: Arc<Mutex<HashMap<String, (u32, Instant)>>>,
}
