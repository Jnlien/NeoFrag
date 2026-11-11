//! Sessions stockées dans `nf_session`, portées de `neofrag/core/session.php`.

use super::{access::Access, ctx::Ctx, password, settings::Settings, user::User};
use crate::{error::AppError, state::AppState};
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{header, HeaderValue},
    middleware::Next,
    response::Response,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use sqlx::Row;
use std::{net::SocketAddr, sync::Arc};

fn is_crawler(user_agent: &str) -> bool {
    let ua = user_agent.to_ascii_lowercase();
    ua.is_empty() || ["bot", "crawl", "spider", "slurp"].iter().any(|n| ua.contains(n))
}

/// Convertit `nf_cookie_expire` (« 1 hour », « 30 minutes »…) en secondes.
fn expire_seconds(setting: &str) -> Option<i64> {
    let mut parts = setting.split_whitespace();
    let amount: i64 = parts.next()?.parse().ok()?;
    let unit = parts.next()?.trim_end_matches('s').to_ascii_lowercase();
    let factor = match unit.as_str() {
        "second" | "sec" => 1,
        "minute" | "min" => 60,
        "hour" => 3600,
        "day" => 86_400,
        "week" => 604_800,
        "month" => 2_592_000,
        "year" => 31_536_000,
        _ => return None,
    };
    (amount > 0).then_some(amount * factor)
}

pub fn cookie_name(settings: &Settings, https: bool) -> String {
    let name = match settings.get("nf_cookie_name") {
        "" => "session",
        name => name,
    };
    if https {
        format!("{name}_https")
    } else {
        name.to_owned()
    }
}

fn session_cookie(state: &AppState, name: String, id: String) -> Cookie<'static> {
    let base = &state.config.server.base_url;
    Cookie::build((name, id))
        .path(if base.is_empty() { "/".to_owned() } else { base.clone() })
        .http_only(true)
        .secure(state.config.server.https)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::days(365))
        .build()
}

async fn load_user(state: &AppState, session_id: &str) -> sqlx::Result<Option<User>> {
    let row = sqlx::query(
        "SELECT u.id, u.username, (u.admin = '1') AS admin, f.path AS avatar, up.sex \
         FROM nf_session s \
         INNER JOIN nf_user u ON u.id = s.user_id AND u.deleted = '0' \
         LEFT JOIN nf_user_profile up ON up.id = u.id \
         LEFT JOIN nf_file f ON f.id = up.avatar \
         WHERE s.id = ?",
    )
    .bind(session_id)
    .fetch_optional(&state.db)
    .await?;

    row.map(|row| {
        Ok(User {
            id: row.try_get("id")?,
            username: super::text::clean(&row.try_get::<String, _>("username")?),
            admin: row.try_get::<i64, _>("admin")? != 0,
            avatar: row.try_get("avatar")?,
            sex: row.try_get("sex")?,
        })
    })
    .transpose()
}

/// Ouvre (ou crée) la session, charge l'utilisateur et ses permissions, puis passe la main.
pub async fn middleware(
    State(state): State<AppState>,
    jar: CookieJar,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let settings = Settings::load(&state.db, &state.config.site.lang).await?;

    let headers = request.headers();
    let header_str = |name: header::HeaderName| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned()
    };
    let user_agent: String = header_str(header::USER_AGENT).chars().take(250).collect();
    let ip = headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| {
            request
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|info| info.0.ip().to_string())
        })
        .unwrap_or_default()
        .chars()
        .take(39)
        .collect::<String>();

    let base = &state.config.server.base_url;
    let path = request.uri().path();
    let path = path.strip_prefix(base.as_str()).unwrap_or(path).trim_matches('/').to_owned();

    let cookie_name = cookie_name(&settings, state.config.server.https);
    let mut session_id = String::new();
    let mut new_cookie = None;

    if !is_crawler(&user_agent) {
        if let Some(seconds) = expire_seconds(settings.get("nf_cookie_expire")) {
            sqlx::query(
                "DELETE FROM nf_session WHERE remember = '0' AND last_activity < DATE_SUB(NOW(), INTERVAL ? SECOND)",
            )
            .bind(seconds)
            .execute(&state.db)
            .await?;
        }

        if let Some(cookie) = jar.get(&cookie_name) {
            let candidate = cookie.value();
            let valid = candidate.len() == 32 && candidate.bytes().all(|b| b.is_ascii_alphanumeric());
            if valid {
                let touched = sqlx::query("UPDATE nf_session SET last_activity = NOW() WHERE id = ?")
                    .bind(candidate)
                    .execute(&state.db)
                    .await?;
                // `rows_affected` vaut 0 si la date n'a pas changé (même seconde) : on vérifie l'existence.
                if touched.rows_affected() > 0
                    || sqlx::query("SELECT 1 FROM nf_session WHERE id = ?")
                        .bind(candidate)
                        .fetch_optional(&state.db)
                        .await?
                        .is_some()
                {
                    session_id = candidate.to_owned();
                }
            }
        }

        if session_id.is_empty() {
            session_id = password::unique_id();
            sqlx::query("INSERT INTO nf_session (id, data) VALUES (?, '')")
                .bind(&session_id)
                .execute(&state.db)
                .await?;
            new_cookie = Some(session_cookie(&state, cookie_name, session_id.clone()));
        }
    }

    let user = if session_id.is_empty() {
        None
    } else {
        load_user(&state, &session_id).await?
    };
    if let Some(user) = &user {
        sqlx::query("UPDATE nf_user SET last_activity_date = NOW() WHERE id = ?")
            .bind(user.id)
            .execute(&state.db)
            .await?;
    }

    let access = Access::load(&state.db, user.as_ref().map(|u| (u.id, u.admin))).await?;

    // Modules désactivés : `nf_addon.data` est un tableau PHP sérialisé (`enabled`).
    let mut disabled = Vec::new();
    let addons = sqlx::query(
        "SELECT a.name, t.name AS kind, IFNULL(a.data, '') AS data FROM nf_addon a \
         INNER JOIN nf_addon_type t ON t.id = a.type_id AND t.name IN ('module', 'widget')",
    )
    .fetch_all(&state.db)
    .await?;
    for row in addons {
        let data: String = row.try_get("data")?;
        let enabled = super::php::unserialize(&data)
            .ok()
            .and_then(|value| value.get("enabled").map(super::php::Value::truthy))
            .unwrap_or(true);
        if !enabled {
            let name: String = row.try_get("name")?;
            // Les widgets désactivés sont notés `widget:<nom>`.
            disabled.push(if row.try_get::<String, _>("kind")? == "widget" { format!("widget:{name}") } else { name });
        }
    }

    let ctx = Arc::new(Ctx {
        state: state.clone(),
        settings,
        session_id,
        user,
        access,
        disabled,
        path,
        ip,
        user_agent,
    });
    let mut response = if crate::modules::settings::closed_for(&ctx) {
        crate::modules::settings::maintenance_page(&ctx)
    } else {
        request.extensions_mut().insert(ctx);
        next.run(request).await
    };
    // Ne pas écraser un cookie de session posé par le handler (connexion).
    if let Some(cookie) = new_cookie {
        if !response.headers().contains_key(header::SET_COOKIE) {
            if let Ok(value) = HeaderValue::from_str(&cookie.to_string()) {
                response.headers_mut().append(header::SET_COOKIE, value);
            }
        }
    }
    Ok(response)
}

/// Connecte l'utilisateur : change l'identifiant de session (anti-fixation), rattache
/// l'utilisateur et journalise dans `nf_session_history`. Renvoie le cookie à poser.
pub async fn login(ctx: &Ctx, user_id: u32, remember: bool) -> sqlx::Result<Cookie<'static>> {
    let state = &ctx.state;
    let new_id = password::unique_id();

    sqlx::query("UPDATE nf_session SET id = ?, user_id = ?, remember = ?, last_activity = NOW() WHERE id = ?")
        .bind(&new_id)
        .bind(user_id)
        .bind(if remember { "1" } else { "0" })
        .bind(&ctx.session_id)
        .execute(&state.db)
        .await?;

    sqlx::query(
        "INSERT INTO nf_session_history (user_id, ip_address, host_name, referer, user_agent) VALUES (?, ?, '', '', ?)",
    )
    .bind(user_id)
    .bind(&ctx.ip)
    .bind(super::text::encode(&ctx.user_agent).chars().take(250).collect::<String>())
    .execute(&state.db)
    .await?;

    let name = cookie_name(&ctx.settings, state.config.server.https);
    Ok(session_cookie(state, name, new_id))
}

pub async fn logout(ctx: &Ctx) -> sqlx::Result<()> {
    sqlx::query("UPDATE nf_session SET user_id = NULL, remember = '0' WHERE id = ?")
        .bind(&ctx.session_id)
        .execute(&ctx.state.db)
        .await?;
    Ok(())
}

/// Nombre de sessions actives (5 dernières minutes).
pub async fn online_count(ctx: &Ctx) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM nf_session WHERE last_activity > DATE_SUB(NOW(), INTERVAL 5 MINUTE)")
        .fetch_one(&ctx.state.db)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expire_setting() {
        assert_eq!(expire_seconds("1 hour"), Some(3600));
        assert_eq!(expire_seconds("30 minutes"), Some(1800));
        assert_eq!(expire_seconds("2 days"), Some(172_800));
        assert_eq!(expire_seconds(""), None);
        assert_eq!(expire_seconds("bientôt"), None);
    }

    #[test]
    fn crawlers() {
        assert!(is_crawler("Mozilla/5.0 (compatible; Googlebot/2.1)"));
        assert!(is_crawler(""));
        assert!(!is_crawler("Mozilla/5.0 (X11; Linux x86_64) Firefox/130.0"));
    }
}
