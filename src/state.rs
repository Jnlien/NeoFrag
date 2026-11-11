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
    /// Tentatives de connexion ratées par adresse IP (anti force brute, en mémoire).
    pub login_attempts: Arc<Mutex<HashMap<String, (u32, Instant)>>>,
}
