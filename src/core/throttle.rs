//! Limitation de débit en mémoire (tentatives de connexion, inscriptions, envois).
//!
//! Volontairement simple : un compteur par clé (`login:<ip>`…) sur une fenêtre de temps.
//! Les compteurs sont perdus au redémarrage et ne sont pas partagés entre instances.

use crate::state::AppState;
use std::time::{Duration, Instant};

/// La clé a-t-elle atteint `max` actions dans la fenêtre en cours ?
pub fn blocked(state: &AppState, key: &str, max: u32, window: Duration) -> bool {
    let counters = state.throttle.lock().unwrap_or_else(|e| e.into_inner());
    counters
        .get(key)
        .is_some_and(|(count, since)| *count >= max && since.elapsed() < window)
}

/// Compte une action pour la clé (et purge les compteurs expirés).
pub fn hit(state: &AppState, key: &str, window: Duration) {
    let mut counters = state.throttle.lock().unwrap_or_else(|e| e.into_inner());
    counters.retain(|_, (_, since)| since.elapsed() < window.max(Duration::from_secs(3600)));
    let entry = counters.entry(key.to_owned()).or_insert((0, Instant::now()));
    if entry.1.elapsed() >= window {
        *entry = (0, Instant::now());
    }
    entry.0 += 1;
}
