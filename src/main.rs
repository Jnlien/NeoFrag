//! NeoFrag — réécriture en Rust du CMS NeoFrag (https://github.com/NeoFrag/NeoFrag).
//!
//! Le serveur parle au même schéma MySQL/MariaDB que NeoFrag Alpha 0.2.3 : on peut le
//! brancher sur la base d'un site existant. Voir README.md et ROADMAP.md.

mod config;
mod core;
mod error;
mod modules;
mod state;
mod widgets;

use crate::{
    config::Config,
    core::{ctx::Ctx, layout, mail::Mailer, session},
    error::AppError,
    modules::{
        account, admin, awards, comments, contact, events, forum, games, members, news, pages, partners, recovery, search, teams, user,
    },
    state::AppState,
};
use axum::{
    extract::{DefaultBodyLimit, Request},
    http::{header, HeaderValue, Method},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Extension, Router,
};
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};
use std::{net::SocketAddr, path::PathBuf, str::FromStr, sync::Arc, time::Duration};
use tower_http::{services::ServeDir, trace::TraceLayer};

/// Page d'accueil : le module ou la page désigné par le réglage `nf_default_page`.
async fn home(Extension(ctx): Extension<Arc<Ctx>>) -> Result<Response, AppError> {
    match ctx.settings.get("nf_default_page") {
        "" | "news" => news::index_page(&ctx, None).await,
        "members" => members::index(Extension(ctx.clone())).await,
        "search" => Ok(Redirect::to(&ctx.url("search")).into_response()),
        name => pages::show(&ctx, name).await,
    }
}

/// Tout ce qui n'est pas une route de module : une page statique (`/reglement`) ou une 404.
async fn fallback(Extension(ctx): Extension<Arc<Ctx>>, request: Request) -> Result<Response, AppError> {
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return layout::not_found(&ctx).await;
    }
    // `/news/` → `/news`
    if request.uri().path().len() > 1 && request.uri().path().ends_with('/') && !ctx.path.is_empty() {
        return Ok(Redirect::permanent(&ctx.url(&ctx.path)).into_response());
    }
    let single_segment = !ctx.path.is_empty() && !ctx.path.contains('/');
    if single_segment && !pages::RESERVED.contains(&ctx.path.as_str()) {
        return pages::show(&ctx, &ctx.path).await;
    }
    layout::not_found(&ctx).await
}

async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("SAMEORIGIN"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("same-origin"));
    response
}

fn router(state: AppState) -> Router {
    let assets = &state.config.site.assets_dir;
    let mut statics = Router::new();
    for dir in ["css", "js", "fonts", "images", "themes", "modules", "widgets"] {
        statics = statics.nest_service(&format!("/{dir}"), ServeDir::new(assets.join(dir)));
    }
    statics = statics.nest_service("/upload", ServeDir::new(&state.config.site.upload_dir));

    let pages = Router::new()
        .route("/", get(home))
        // Actualités
        .route("/news", get(news::index))
        .route("/news/page/{page}", get(news::index_paged))
        .route("/news/{id}/{slug}", get(news::show))
        .route("/news/tag/{tag}", get(news::tag))
        .route("/news/tag/{tag}/page/{page}", get(news::tag_paged))
        .route("/news/category/{id}/{name}", get(news::category))
        .route("/news/category/{id}/{name}/page/{page}", get(news::category_paged))
        // Membres et utilisateur
        .route("/members", get(members::index))
        .route("/members/page/{page}", get(members::index_paged))
        .route("/user", get(user::index))
        .route("/user/login", get(user::login_get).post(user::login_post))
        .route("/user/logout", post(user::logout))
        .route("/user/register", get(account::register_get).post(account::register_post))
        .route("/user/edit", get(account::edit_get).post(account::edit_post))
        .route("/user/avatar", post(account::avatar_post))
        .route("/user/avatar/delete", post(account::avatar_delete))
        .route("/user/account", get(account::account_get).post(account::account_post))
        .route("/user/account/password", post(account::password_post))
        .route("/user/validation/{token}", get(recovery::validation))
        .route("/user/lost-password", get(recovery::lost_get).post(recovery::lost_post))
        .route("/user/lost-password/{token}", get(recovery::reset_get).post(recovery::reset_post))
        .route("/contact", get(contact::index).post(contact::send))
        // Équipes, partenaires, palmarès, événements
        .route("/teams", get(teams::index))
        .route("/teams/{id}/{name}", get(teams::show))
        .route("/partners", get(partners::index))
        .route("/partners/{id}/{name}", get(partners::visit))
        .route("/awards", get(awards::index))
        .route("/awards/{id}/{slug}", get(awards::show))
        .route("/awards/{kind}/{id}/{slug}", get(awards::filtered))
        .route("/events", get(events::index))
        .route("/events/participant/{id}", post(events::participate))
        .route("/events/{*rest}", get(events::route))
        // Forum
        .route("/forum", get(forum::index))
        .route("/forum/mark-all-as-read", post(forum::mark_all_read))
        .route("/forum/new/{forum_id}", post(forum::new_topic))
        .route("/forum/reply/{topic_id}", post(forum::reply))
        .route("/forum/message/edit/{id}", post(forum::edit_message))
        .route("/forum/message/delete/{id}", post(forum::delete_message))
        .route("/forum/moderate/{action}/{id}", post(forum::moderate))
        .route("/forum/{*rest}", get(forum::route))
        .route("/comments/add", post(comments::add))
        .route("/comments/delete/{id}", post(comments::delete))
        .route("/user/{id}/{slug}", get(user::profile))
        .route("/search", get(search::index))
        // Administration
        .route("/admin", get(admin::dashboard))
        .route("/admin/news", get(admin::news_list))
        .route("/admin/news/add", get(admin::news_add_get).post(admin::news_add_post))
        .route("/admin/news/{id}/{slug}", get(admin::news_edit_get).post(admin::news_edit_post))
        .route("/admin/news/delete/{id}", post(admin::news_delete))
        .route(
            "/admin/news/categories/add",
            get(admin::category_add_get).post(admin::category_add_post),
        )
        .route(
            "/admin/news/categories/{id}/{name}",
            get(admin::category_edit_get).post(admin::category_edit_post),
        )
        .route("/admin/news/categories/delete/{id}", post(admin::category_delete))
        .route("/admin/pages", get(admin::pages_list))
        .route("/admin/pages/add", get(admin::page_add_get).post(admin::page_add_post))
        .route("/admin/pages/{id}/{slug}", get(admin::page_edit_get).post(admin::page_edit_post))
        .route("/admin/pages/delete/{id}", post(admin::page_delete))
        .route("/admin/games", get(games::admin_list))
        .route("/admin/games/add", get(games::admin_add_get).post(games::admin_add_post))
        .route("/admin/games/delete/{id}", post(games::admin_delete))
        .route("/admin/games/{id}/{name}", get(games::admin_edit_get).post(games::admin_edit_post))
        .route("/admin/games/{id}/{kind}/add", post(games::admin_sub_add))
        .route("/admin/games/{kind}/delete/{id}", post(games::admin_sub_delete))
        .route("/admin/teams", get(teams::admin_list))
        .route("/admin/teams/add", get(teams::admin_add_get).post(teams::admin_add_post))
        .route("/admin/teams/delete/{id}", post(teams::admin_delete))
        .route("/admin/teams/roles/add", post(teams::admin_role_add))
        .route("/admin/teams/roles/delete/{id}", post(teams::admin_role_delete))
        .route("/admin/teams/{id}/{name}", get(teams::admin_edit_get).post(teams::admin_edit_post))
        .route("/admin/teams/{id}/players/add", post(teams::admin_player_add))
        .route("/admin/teams/{id}/players/delete/{user_id}", post(teams::admin_player_delete))
        .route("/admin/partners", get(partners::admin_list))
        .route("/admin/partners/add", get(partners::admin_add_get).post(partners::admin_add_post))
        .route("/admin/partners/delete/{id}", post(partners::admin_delete))
        .route("/admin/partners/{id}/{name}", get(partners::admin_edit_get).post(partners::admin_edit_post))
        .route("/admin/awards", get(awards::admin_list))
        .route("/admin/awards/add", get(awards::admin_add_get).post(awards::admin_add_post))
        .route("/admin/awards/delete/{id}", post(awards::admin_delete))
        .route("/admin/awards/{id}/{slug}", get(awards::admin_edit_get).post(awards::admin_edit_post))
        .route("/admin/events", get(events::admin_list))
        .route("/admin/events/add", get(events::admin_add_get).post(events::admin_add_post))
        .route("/admin/events/delete/{id}", post(events::admin_delete))
        .route("/admin/events/types/add", post(events::admin_type_add))
        .route("/admin/events/types/delete/{id}", post(events::admin_type_delete))
        .route("/admin/events/opponents/add", post(events::admin_opponent_add))
        .route("/admin/events/opponents/delete/{id}", post(events::admin_opponent_delete))
        .route("/admin/events/rounds/delete/{id}", post(events::admin_round_delete))
        .route("/admin/events/{id}/{slug}", get(events::admin_edit_get).post(events::admin_edit_post))
        .route("/admin/events/{id}/rounds/add", post(events::admin_round_add))
        .route("/admin/events/{id}/participants/add", post(events::admin_participant_add))
        .route("/admin/events/{id}/participants/delete/{user_id}", post(events::admin_participant_delete))
        .route("/admin/forum", get(forum::admin_list))
        .route("/admin/forum/add", get(forum::admin_add_get).post(forum::admin_add_post))
        .route("/admin/forum/delete/{id}", post(forum::admin_delete))
        .route("/admin/forum/categories/add", post(forum::admin_category_add))
        .route("/admin/forum/categories/delete/{id}", post(forum::admin_category_delete))
        .route("/admin/forum/{id}/{slug}", get(forum::admin_edit_get).post(forum::admin_edit_post))
        .route("/admin/user", get(admin::users))
        .route("/admin/user/role/{id}", post(admin::user_role))
        .route("/admin/user/delete/{id}", post(admin::user_delete))
        .route("/admin/settings", get(admin::settings_get).post(admin::settings_post))
        .fallback(fallback)
        // Corps de requête : un fichier envoyé plus une marge pour les autres champs.
        .layer(DefaultBodyLimit::max(
            state.config.site.max_upload_mb.saturating_mul(1024 * 1024).saturating_add(1024 * 1024),
        ))
        .layer(middleware::from_fn_with_state(state.clone(), session::middleware));

    let app = statics
        .merge(pages)
        .layer(middleware::from_fn(security_headers))
        .layer(TraceLayer::new_for_http());

    let base = &state.config.server.base_url;
    if base.is_empty() {
        app
    } else {
        Router::new().nest(base, app)
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("arrêt demandé");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "neofrag=info,tower_http=info".into()),
        )
        .init();

    let config_path = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("NEOFRAG_CONFIG").ok())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("neofrag.toml"));
    let config = Config::load(&config_path)?;

    // Fuseau du serveur de base de données (et non UTC forcé) : les dates s'affichent
    // comme dans l'installation PHP d'origine.
    let options = MySqlConnectOptions::from_str(&config.database.url)?.timezone(None::<String>);
    let db = MySqlPoolOptions::new()
        .max_connections(config.database.max_connections)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(options)
        .await
        .map_err(|e| anyhow::anyhow!("connexion à la base impossible : {e}"))?;

    sqlx::query("SELECT 1 FROM nf_settings LIMIT 1")
        .fetch_optional(&db)
        .await
        .map_err(|e| {
            anyhow::anyhow!("la base ne contient pas le schéma NeoFrag (importez legacy/DATABASE.sql) : {e}")
        })?;

    let bind: SocketAddr = config
        .server
        .bind
        .parse()
        .map_err(|e| anyhow::anyhow!("server.bind invalide : {e}"))?;
    let mailer = Mailer::from_config(&config)
        .map_err(|e| anyhow::anyhow!("configuration [email] invalide : {e}"))?
        .map(Arc::new);
    if mailer.is_none() {
        tracing::info!("envoi d'emails désactivé (mot de passe oublié, validation d'inscription, contact)");
    }
    let state = AppState {
        db,
        mailer,
        config: Arc::new(config),
        throttle: Arc::default(),
    };

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!("NeoFrag à l'écoute sur http://{bind}{}", state.config.server.base_url);
    axum::serve(listener, router(state).into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}
