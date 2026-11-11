//! Module Événements : agenda, matchs (adversaire, manches, scores) et participants.
//! Porté de `modules/events`.
//!
//! Un type d'événement « lié aux équipes » (`nf_events_types.type = 1`) peut porter un
//! match. La visibilité d'un type suit la permission `events / access_events_type`.

use super::{
    admin::{self, admin_guard, Ctxt, CsrfOnly, Notice, Reply},
    comments, teams,
};
use crate::{
    core::{
        bbcode, countries,
        ctx::Ctx,
        files::{Fit, FormData},
        form::{self, checkbox, fits, input, textarea},
        layout::{self, Page},
        pagination::Pagination,
        panel::{Block, Panel},
        text, user,
    },
    error::AppError,
};
use axum::{
    extract::{Multipart, Path, Query},
    response::{IntoResponse, Redirect},
    Extension, Form,
};
use chrono::{NaiveDateTime, Timelike};
use serde::Deserialize;
use sqlx::Row;

pub const TITLE: &str = "Événements";
pub const ICON: &str = "far fa-calendar-alt";
const PER_PAGE: i64 = 10;

/// Statuts de participation, dans l'ordre de `nf_events_participants.status`.
const STATUSES: [(&str, &str, &str); 4] = [
    ("En attente", "info", "fas fa-question"),
    ("Présent", "success", "fas fa-check"),
    ("Absent", "danger", "fas fa-times"),
    ("Peut-être", "warning", "fas fa-ellipsis-h"),
];
const COLORS: [&str; 7] = ["default", "primary", "success", "info", "warning", "danger", "dark"];

pub struct Event {
    pub event_id: u32,
    type_id: u32,
    pub title: String,
    description: String,
    private_description: String,
    location: String,
    pub date: NaiveDateTime,
    date_end: Option<NaiveDateTime>,
    published: bool,
    image: Option<String>,
    image_id: Option<u32>,
    type_title: String,
    color: String,
    icon: String,
}

impl Event {
    pub fn path(&self) -> String {
        format!("events/{}/{}", self.event_id, text::url_title(&self.title))
    }

    fn type_label(&self) -> String {
        format!(
            "<span class=\"badge badge-{}\">{} {}</span>",
            text::css_classes(if self.color == "default" { "secondary" } else { &self.color }),
            text::icon(&self.icon),
            self.type_title
        )
    }
}

const SELECT: &str = "SELECT e.event_id, e.type_id, e.title, e.description, e.private_description, e.location, \
        CAST(e.date AS DATETIME) AS date, CAST(e.date_end AS DATETIME) AS date_end, (e.published = '1') AS is_published, \
        e.image_id, f.path AS image, t.title AS type_title, t.color, t.icon \
     FROM nf_events e INNER JOIN nf_events_types t ON t.type_id = e.type_id \
     LEFT JOIN nf_file f ON f.id = e.image_id \
     LEFT JOIN nf_events_matches m ON m.event_id = e.event_id";

fn from_row(row: &sqlx::mysql::MySqlRow) -> sqlx::Result<Event> {
    Ok(Event {
        event_id: row.try_get("event_id")?,
        type_id: row.try_get("type_id")?,
        title: text::clean(&row.try_get::<String, _>("title")?),
        description: text::clean(&row.try_get::<String, _>("description")?),
        private_description: text::clean(&row.try_get::<String, _>("private_description")?),
        location: text::clean(&row.try_get::<String, _>("location")?),
        date: row.try_get("date")?,
        date_end: row.try_get("date_end")?,
        published: row.try_get::<i64, _>("is_published")? != 0,
        image: row.try_get("image")?,
        image_id: row.try_get("image_id")?,
        type_title: text::clean(&row.try_get::<String, _>("type_title")?),
        color: row.try_get("color")?,
        icon: row.try_get("icon")?,
    })
}

async fn find(ctx: &Ctx, event_id: u32) -> Result<Option<Event>, AppError> {
    let row = sqlx::query(&format!("{SELECT} WHERE e.event_id = ?"))
        .bind(event_id)
        .fetch_optional(&ctx.state.db)
        .await?;
    Ok(row.as_ref().map(from_row).transpose()?)
}

/// Événement publié et visible par le visiteur (utilisé aussi par les commentaires).
pub async fn find_visible(ctx: &Ctx, event_id: u32) -> Result<Option<Event>, AppError> {
    Ok(find(ctx, event_id)
        .await?
        .filter(|e| e.published && ctx.access.can("events", "access_events_type", e.type_id)))
}

/// Types que le visiteur a le droit de voir, sous forme de liste SQL (`1,2,5`).
async fn visible_types(ctx: &Ctx) -> Result<String, AppError> {
    let ids: Vec<u32> = sqlx::query_scalar("SELECT type_id FROM nf_events_types")
        .fetch_all(&ctx.state.db)
        .await?;
    let visible: Vec<String> = ids
        .into_iter()
        .filter(|id| ctx.access.can("events", "access_events_type", *id))
        .map(|id| id.to_string())
        .collect();
    // Des entiers issus de la base : sans danger dans la requête. `0` ne correspond à rien.
    Ok(if visible.is_empty() { "0".to_owned() } else { visible.join(",") })
}

struct Round {
    round_id: u32,
    map: Option<String>,
    score1: u32,
    score2: u32,
}

struct Match {
    team_id: u32,
    team_title: String,
    team_name: String,
    opponent_id: u32,
    opponent_title: String,
    opponent_image: Option<String>,
    opponent_website: String,
    opponent_country: String,
    mode_id: Option<u32>,
    mode: Option<String>,
    webtv: String,
    website: String,
    rounds: Vec<Round>,
}

impl Match {
    /// Score global : celui de la manche s'il n'y en a qu'une, sinon le nombre de manches gagnées.
    fn score(&self) -> Option<(u32, u32)> {
        match self.rounds.as_slice() {
            [] => None,
            [only] => Some((only.score1, only.score2)),
            rounds => Some((
                rounds.iter().filter(|r| r.score1 > r.score2).count() as u32,
                rounds.iter().filter(|r| r.score2 > r.score1).count() as u32,
            )),
        }
    }

    fn score_html(&self) -> String {
        match self.score() {
            Some((ours, theirs)) => {
                let color = match ours.cmp(&theirs) {
                    std::cmp::Ordering::Greater => "success",
                    std::cmp::Ordering::Less => "danger",
                    std::cmp::Ordering::Equal => "warning",
                };
                format!("<span class=\"badge badge-{color}\">{ours} - {theirs}</span>")
            }
            None => "<span class=\"badge badge-secondary\">vs</span>".to_owned(),
        }
    }
}

async fn match_of(ctx: &Ctx, event_id: u32) -> Result<Option<Match>, AppError> {
    let db = &ctx.state.db;
    let row = sqlx::query(
        "SELECT m.team_id, tl.title AS team_title, t.name AS team_name, m.opponent_id, o.title AS opponent_title, \
                fo.path AS opponent_image, o.website AS opponent_website, o.country, m.mode_id, gm.title AS mode, m.webtv, m.website \
         FROM nf_events_matches m \
         INNER JOIN nf_teams t ON t.team_id = m.team_id \
         INNER JOIN nf_teams_lang tl ON tl.team_id = t.team_id AND tl.lang = ? \
         INNER JOIN nf_events_matches_opponents o ON o.opponent_id = m.opponent_id \
         LEFT JOIN nf_file fo ON fo.id = o.image_id \
         LEFT JOIN nf_games_modes gm ON gm.mode_id = m.mode_id \
         WHERE m.event_id = ?",
    )
    .bind(ctx.lang())
    .bind(event_id)
    .fetch_optional(db)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let rounds = sqlx::query(
        "SELECT r.round_id, gm.title AS map, CAST(r.score1 AS SIGNED) AS score1, CAST(r.score2 AS SIGNED) AS score2 FROM nf_events_matches_rounds r \
         LEFT JOIN nf_games_maps gm ON gm.map_id = r.map_id WHERE r.event_id = ? ORDER BY r.round_id",
    )
    .bind(event_id)
    .fetch_all(db)
    .await?;
    Ok(Some(Match {
        team_id: row.try_get("team_id")?,
        team_title: text::clean(&row.try_get::<String, _>("team_title")?),
        team_name: row.try_get("team_name")?,
        opponent_id: row.try_get("opponent_id")?,
        opponent_title: text::clean(&row.try_get::<String, _>("opponent_title")?),
        opponent_image: row.try_get("opponent_image")?,
        opponent_website: row.try_get("opponent_website")?,
        opponent_country: row.try_get("country")?,
        mode_id: row.try_get("mode_id")?,
        mode: row.try_get::<Option<String>, _>("mode")?.map(|m| text::clean(&m)),
        webtv: row.try_get("webtv")?,
        website: row.try_get("website")?,
        rounds: rounds
            .iter()
            .map(|r| {
                Ok(Round {
                    round_id: r.try_get("round_id")?,
                    map: r.try_get::<Option<String>, _>("map")?.map(|m| text::clean(&m)),
                    score1: r.try_get::<i64, _>("score1")?.max(0) as u32,
                    score2: r.try_get::<i64, _>("score2")?.max(0) as u32,
                })
            })
            .collect::<sqlx::Result<_>>()?,
    }))
}

/// Lien externe sûr (http/https uniquement), ou rien.
fn external_link(url: &str, label: &str, icon: &str) -> String {
    let url = text::decode(url);
    if url.starts_with("https://") || url.starts_with("http://") {
        format!(
            "<a class=\"btn btn-light btn-sm\" href=\"{}\" rel=\"nofollow noopener\" target=\"_blank\">{} {label}</a> ",
            text::encode(&url),
            text::icon(icon)
        )
    } else {
        String::new()
    }
}

fn valid_url(url: &str) -> bool {
    url.is_empty()
        || ((url.starts_with("https://") || url.starts_with("http://"))
            && fits(url, 100)
            && !url.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '<' | '>')))
}

fn versus(ctx: &Ctx, game: &Match) -> String {
    format!(
        "<a href=\"{}\">{}</a> {} <b>{}</b>",
        ctx.url(&format!("teams/{}/{}", game.team_id, game.team_name)),
        game.team_title,
        game.score_html(),
        game.opponent_title
    )
}

/// Ligne d'un événement dans une liste.
async fn item_html(ctx: &Ctx, event: &Event) -> Result<String, AppError> {
    let game = match_of(ctx, event.event_id).await?;
    Ok(format!(
        "<li class=\"list-group-item\"><div class=\"float-right text-muted\">{} {}</div>{} <a href=\"{}\"><b>{}</b></a>{}</li>",
        text::icon("far fa-clock"),
        text::date_time(&event.date),
        event.type_label(),
        ctx.url(&event.path()),
        event.title,
        match &game {
            Some(game) => format!("<div class=\"mt-1\">{}</div>", versus(ctx, game)),
            None => String::new(),
        }
    ))
}

enum Filter {
    All,
    Upcoming,
    Standards,
    Matches,
    Type(u32),
    Team(u32),
}

async fn list(ctx: &Ctx, filter: &Filter, limit: u32, offset: u32) -> Result<(Vec<Event>, i64), AppError> {
    let types = visible_types(ctx).await?;
    let (condition, order) = match filter {
        Filter::All => ("1 = 1", "e.date DESC"),
        Filter::Upcoming => ("e.date > NOW()", "e.date ASC"),
        Filter::Standards => ("m.event_id IS NULL", "e.date DESC"),
        Filter::Matches => ("m.event_id IS NOT NULL", "e.date DESC"),
        Filter::Type(_) => ("e.type_id = ?", "e.date DESC"),
        Filter::Team(_) => ("m.team_id = ?", "e.date DESC"),
    };
    let where_sql = format!("WHERE e.published = '1' AND e.type_id IN ({types}) AND {condition}");
    let count_sql = format!(
        "SELECT COUNT(*) FROM nf_events e LEFT JOIN nf_events_matches m ON m.event_id = e.event_id {where_sql}"
    );
    let list_sql = format!("{SELECT} {where_sql} ORDER BY {order} LIMIT ? OFFSET ?");
    let mut count = sqlx::query_scalar::<_, i64>(&count_sql);
    let mut query = sqlx::query(&list_sql);
    if let Filter::Type(id) | Filter::Team(id) = filter {
        count = count.bind(*id);
        query = query.bind(*id);
    }
    let total = count.fetch_one(&ctx.state.db).await?;
    let rows = query.bind(limit).bind(offset).fetch_all(&ctx.state.db).await?;
    Ok((rows.iter().map(from_row).collect::<sqlx::Result<_>>()?, total))
}

async fn listing(ctx: &Ctx, filter: Filter, base_path: String, page_no: u32, heading: String) -> Reply {
    let (_, total) = list(ctx, &filter, 0, 0).await?;
    let pagination = Pagination::new(page_no, PER_PAGE, total);
    if pagination.out_of_range() {
        return Ok(Redirect::to(&ctx.url(&base_path)).into_response());
    }
    let (events, _) = list(ctx, &filter, pagination.per_page, pagination.offset()).await?;

    let tab = |path: &str, label: &str| {
        format!(
            "<li class=\"nav-item\"><a class=\"nav-link{}\" href=\"{}\">{label}</a></li>",
            if path == base_path { " active" } else { "" },
            ctx.url(path)
        )
    };
    let tabs = format!(
        "<ul class=\"nav\">{}{}{}{}</ul>",
        tab("events", "Tous"),
        tab("events/upcoming", "À venir"),
        tab("events/matches", "Matchs"),
        tab("events/standards", "Autres"),
    );
    let mut items = String::new();
    for event in &events {
        items.push_str(&item_html(ctx, event).await?);
    }
    let body = if items.is_empty() {
        "<div class=\"card-body text-center\">Aucun événement</div>".to_owned()
    } else {
        format!("<ul class=\"list-group list-group-flush\">{items}</ul>")
    };
    let mut blocks = vec![
        Block::Panel(Panel::new().style("mb-4").body_raw(tabs)),
        Block::Panel(Panel::new().heading(heading, ICON).body_raw(body)),
    ];
    if let Some(panel) = pagination.panel(&ctx.url(&base_path)) {
        blocks.push(Block::Panel(panel));
    }
    layout::render(ctx, Page::new("events", TITLE, ICON).blocks(&blocks)).await
}

struct Participant {
    user_id: u32,
    username: String,
    status: usize,
}

async fn participants(ctx: &Ctx, event_id: u32) -> Result<Vec<Participant>, AppError> {
    let rows = sqlx::query(
        "SELECT u.id, u.username, CAST(ep.status AS SIGNED) AS status FROM nf_events_participants ep \
         INNER JOIN nf_user u ON u.id = ep.user_id AND u.deleted = '0' \
         WHERE ep.event_id = ? ORDER BY ep.status, u.username",
    )
    .bind(event_id)
    .fetch_all(&ctx.state.db)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(Participant {
                user_id: row.try_get("id")?,
                username: row.try_get("username")?,
                status: (row.try_get::<i64, _>("status")?.clamp(0, 3)) as usize,
            })
        })
        .collect::<sqlx::Result<_>>()
        .map_err(Into::into)
}

async fn show(ctx: &Ctx, event_id: u32, slug: &str) -> Reply {
    let event = match find_visible(ctx, event_id).await? {
        Some(event) if text::url_title(&event.title) == slug => event,
        _ => return layout::not_found(ctx).await,
    };
    let game = match_of(ctx, event_id).await?;
    let people = participants(ctx, event_id).await?;
    let me = ctx.user.as_ref().map(|u| u.id);
    let invited = people.iter().any(|p| Some(p.user_id) == me);

    let image = match &event.image {
        Some(path) => format!("<img class=\"card-img-top\" src=\"{}\" alt=\"\" />", text::encode(&ctx.url(path))),
        None => String::new(),
    };
    let dates = match &event.date_end {
        Some(end) => format!("{} → {}", text::date_time(&event.date), text::date_time(end)),
        None => text::date_time(&event.date),
    };
    let mut body = format!(
        "{image}<div class=\"card-body\"><h4 class=\"card-title\">{}</h4><p>{} <span class=\"text-muted ml-2\">{} {dates}</span>{}</p>{}</div>",
        event.title,
        event.type_label(),
        text::icon("far fa-clock"),
        if event.location.is_empty() {
            String::new()
        } else {
            format!(" <span class=\"text-muted ml-2\">{} {}</span>", text::icon("fas fa-map-marker-alt"), event.location)
        },
        bbcode::render(&event.description),
    );
    if !event.private_description.is_empty() && (invited || ctx.access.admin()) {
        body.push_str(&format!(
            "<div class=\"card-body border-top\"><h6>{} Informations réservées aux participants</h6>{}</div>",
            text::icon("fas fa-lock"),
            bbcode::render(&event.private_description)
        ));
    }
    let mut blocks = vec![Block::Panel(Panel::new().body_raw(body))];

    if let Some(game) = &game {
        let logo = match &game.opponent_image {
            Some(path) => format!(
                "<img src=\"{}\" style=\"max-height: 35px; max-width: 50px;\" alt=\"\" /> ",
                text::encode(&ctx.url(path))
            ),
            None => String::new(),
        };
        let links = format!(
            "{}{}{}",
            external_link(&game.opponent_website, "Site de l'adversaire", "fas fa-globe"),
            external_link(&game.webtv, "WebTV", "fas fa-tv"),
            external_link(&game.website, "Site du match", "fas fa-external-link-alt")
        );
        let mut html = format!(
            "<div class=\"card-body text-center\"><h4 class=\"mb-2\">{logo}{}</h4>{}{}{links}</div>",
            versus(ctx, game),
            match countries::name(&game.opponent_country) {
                Some(country) => format!("<span class=\"text-muted mr-2\">{} {}</span>", text::icon("fas fa-flag"), text::encode(country)),
                None => String::new(),
            },
            match &game.mode {
                Some(mode) => format!("<span class=\"badge badge-dark mr-2\">{mode}</span>"),
                None => String::new(),
            },
        );
        if !game.rounds.is_empty() {
            let rows: String = game
                .rounds
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    format!(
                        "<tr><td>Manche {}</td><td>{}</td><td class=\"text-right\"><b>{} - {}</b></td></tr>",
                        i + 1,
                        r.map.as_deref().unwrap_or(""),
                        r.score1,
                        r.score2
                    )
                })
                .collect();
            html.push_str(&format!("<table class=\"table mb-0\"><tbody>{rows}</tbody></table>"));
        }
        blocks.push(Block::Panel(Panel::new().heading("Match", "fas fa-crosshairs").body_raw(html)));
    }

    if !people.is_empty() {
        let mut rows = String::new();
        for p in &people {
            let (label, color, icon) = STATUSES[p.status];
            let mut status = format!("<span class=\"badge badge-{color}\">{} {label}</span>", text::icon(icon));
            if Some(p.user_id) == me {
                // Le membre invité choisit sa réponse.
                status = (1..STATUSES.len())
                    .map(|i| {
                        let (label, color, icon) = STATUSES[i];
                        format!(
                            "<form method=\"post\" action=\"{}\" class=\"d-inline\">{}<input type=\"hidden\" name=\"status\" value=\"{i}\" />\
                             <button type=\"submit\" class=\"btn btn-sm btn-{}{color}\">{} {label}</button></form> ",
                            ctx.url(&format!("events/participant/{event_id}")),
                            form::csrf_field(ctx),
                            if i == p.status { "" } else { "outline-" },
                            text::icon(icon),
                        )
                    })
                    .collect();
            }
            rows.push_str(&format!(
                "<li class=\"list-group-item\"><div class=\"float-right\">{status}</div>{}</li>",
                user::link(ctx.base(), p.user_id, &p.username)
            ));
        }
        let present = people.iter().filter(|p| p.status == 1).count();
        blocks.push(Block::Panel(
            Panel::new()
                .heading(
                    format!("Participants <span class=\"badge badge-secondary float-right\">{present} / {}</span>", people.len()),
                    "fas fa-users",
                )
                .body_raw(format!("<ul class=\"list-group list-group-flush\">{rows}</ul>")),
        ));
    }
    if ctx.enabled("comments") {
        blocks.push(Block::Html(comments::panel(ctx, "events", event_id).await?));
    }

    let page = Page::new("events", TITLE, ICON)
        .method("_event")
        .title(event.title.clone())
        .crumb(event.title.clone(), event.path())
        .blocks(&blocks);
    layout::render(ctx, page).await
}

pub async fn index(Extension(ctx): Ctxt) -> Reply {
    dispatch(&ctx, "").await
}

/// Toutes les URL `events/...` (liste, filtres, pagination, fiche).
pub async fn route(Extension(ctx): Ctxt, Path(rest): Path<String>) -> Reply {
    dispatch(&ctx, &rest).await
}

async fn dispatch(ctx: &Ctx, rest: &str) -> Reply {
    if !ctx.enabled("events") {
        return layout::not_found(ctx).await;
    }
    let mut segments: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    let mut page_no = 1;
    if let [.., "page", n] = segments.as_slice() {
        match n.parse::<u32>() {
            Ok(n) if n >= 1 => page_no = n,
            _ => return layout::not_found(ctx).await,
        }
        segments.truncate(segments.len() - 2);
    }
    match segments.as_slice() {
        [] => listing(ctx, Filter::All, "events".into(), page_no, TITLE.into()).await,
        ["upcoming"] => listing(ctx, Filter::Upcoming, "events/upcoming".into(), page_no, "Événements à venir".into()).await,
        ["standards"] => listing(ctx, Filter::Standards, "events/standards".into(), page_no, "Autres événements".into()).await,
        ["matches"] => listing(ctx, Filter::Matches, "events/matches".into(), page_no, "Matchs".into()).await,
        ["type", id, slug] => {
            let Ok(id) = id.parse::<u32>() else {
                return layout::not_found(ctx).await;
            };
            let title: Option<String> = sqlx::query_scalar("SELECT title FROM nf_events_types WHERE type_id = ?")
                .bind(id)
                .fetch_optional(&ctx.state.db)
                .await?;
            match title.map(|t| text::clean(&t)) {
                Some(title) if ctx.access.can("events", "access_events_type", id) => {
                    listing(ctx, Filter::Type(id), format!("events/type/{id}/{slug}"), page_no, title).await
                }
                _ => layout::not_found(ctx).await,
            }
        }
        ["team", id, slug] => match id.parse::<u32>() {
            Ok(id) => match teams::find(ctx, id).await? {
                Some(team) => {
                    listing(ctx, Filter::Team(id), format!("events/team/{id}/{slug}"), page_no, format!("Matchs — {}", team.title)).await
                }
                None => layout::not_found(ctx).await,
            },
            Err(_) => layout::not_found(ctx).await,
        },
        [id, slug] if page_no == 1 => match id.parse::<u32>() {
            Ok(id) => show(ctx, id, slug).await,
            Err(_) => layout::not_found(ctx).await,
        },
        _ => layout::not_found(ctx).await,
    }
}

#[derive(Deserialize)]
pub struct StatusForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    status: u8,
}

/// Réponse d'un membre invité (présent / absent / peut-être).
pub async fn participate(Extension(ctx): Ctxt, Path(event_id): Path<u32>, Form(form): Form<StatusForm>) -> Reply {
    let Some(current) = &ctx.user else {
        return layout::forbidden(&ctx).await;
    };
    let Some(event) = find_visible(&ctx, event_id).await? else {
        return layout::not_found(&ctx).await;
    };
    if ctx.csrf_ok(&form.csrf) && (1..=3).contains(&form.status) {
        // UPDATE seul : on ne s'invite pas soi-même à un événement.
        sqlx::query("UPDATE nf_events_participants SET status = ? WHERE event_id = ? AND user_id = ?")
            .bind(form.status)
            .bind(event_id)
            .bind(current.id)
            .execute(&ctx.state.db)
            .await?;
    }
    Ok(Redirect::to(&ctx.url(&event.path())).into_response())
}

/// Derniers matchs d'une équipe, pour sa fiche.
pub async fn team_matches(ctx: &Ctx, team_id: u32) -> Result<String, AppError> {
    if !ctx.enabled("events") {
        return Ok(String::new());
    }
    let (events, _) = list(ctx, &Filter::Team(team_id), 5, 0).await?;
    if events.is_empty() {
        return Ok(String::new());
    }
    let mut items = String::new();
    for event in &events {
        items.push_str(&item_html(ctx, event).await?);
    }
    Ok(Panel::new()
        .heading("Derniers matchs", "fas fa-crosshairs")
        .body_raw(format!("<ul class=\"list-group list-group-flush\">{items}</ul>"))
        .render())
}

/// Widgets « Événements » : prochains événements (`index`, `upcoming`) ou derniers matchs (`matches`).
pub async fn widget(ctx: &Ctx, kind: &str) -> Result<Vec<Block>, AppError> {
    let (filter, title, empty) = match kind {
        "matches" => (Filter::Matches, "Derniers matchs", "Aucun match pour le moment"),
        _ => (Filter::Upcoming, "Prochains événements", "Aucun événement à venir"),
    };
    let (events, _) = list(ctx, &filter, 5, 0).await?;
    let panel = Panel::new().heading(title, ICON);
    if events.is_empty() {
        return Ok(vec![Block::Panel(panel.body(empty))]);
    }
    let mut items = String::new();
    for event in &events {
        items.push_str(&item_html(ctx, event).await?);
    }
    let footer = format!(
        "<a href=\"{}\">{} Voir tous les événements</a>",
        ctx.url("events"),
        text::icon("far fa-arrow-alt-circle-right")
    );
    Ok(vec![Block::Panel(
        panel
            .body_raw(format!("<ul class=\"list-group list-group-flush\">{items}</ul>"))
            .footer(footer, true),
    )])
}

// ---------------------------------------------------------------------------
// Administration

pub async fn admin_list(Extension(ctx): Ctxt, Query(notice): Query<Notice>) -> Reply {
    admin_guard!(ctx, "events");
    let db = &ctx.state.db;
    let rows = sqlx::query(&format!("{SELECT} ORDER BY e.date DESC LIMIT 500")).fetch_all(db).await?;
    let mut body = String::new();
    for row in &rows {
        let event = from_row(row)?;
        body.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td>{}</tr>",
            event.title,
            event.type_label(),
            text::date_time(&event.date),
            admin::badge(event.published, "Publié", "Brouillon"),
            admin::row_actions(&ctx, &format!("admin/{}", event.path()), &format!("admin/events/delete/{}", event.event_id), "Supprimer cet événement ?"),
        ));
    }

    let types = sqlx::query("SELECT type_id, CAST(type AS SIGNED) AS type, title, color, icon FROM nf_events_types ORDER BY title")
        .fetch_all(db)
        .await?;
    let mut type_rows = String::new();
    for row in &types {
        type_rows.push_str(&format!(
            "<tr><td>{} {}</td><td>{}</td><td class=\"text-right\">{}</td></tr>",
            text::icon(&row.try_get::<String, _>("icon")?),
            text::clean(&row.try_get::<String, _>("title")?),
            if row.try_get::<i64, _>("type")? == 1 { "Équipes" } else { "Standard" },
            admin::delete_button(&ctx, &format!("admin/events/types/delete/{}", row.try_get::<u32, _>("type_id")?), "Supprimer ce type ET tous ses événements ?"),
        ));
    }
    let color_options: String = COLORS.iter().map(|c| format!("<option>{c}</option>")).collect();
    let type_form = format!(
        "<form method=\"post\" action=\"{}\">{}<input type=\"text\" class=\"form-control mb-2\" name=\"title\" maxlength=\"100\" placeholder=\"Titre\" required />\
         <input type=\"text\" class=\"form-control mb-2\" name=\"icon\" maxlength=\"50\" placeholder=\"Icône (ex. fas fa-trophy)\" />\
         <select class=\"form-control mb-2\" name=\"color\">{color_options}</select>{}\
         <button type=\"submit\" class=\"btn btn-primary\">Ajouter</button></form>",
        ctx.url("admin/events/types/add"),
        form::csrf_field(&ctx),
        checkbox("Lié aux équipes (peut porter un match)", "team", false),
    );

    let opponents = sqlx::query("SELECT opponent_id, title, country FROM nf_events_matches_opponents ORDER BY title")
        .fetch_all(db)
        .await?;
    let mut opponent_rows = String::new();
    for row in &opponents {
        opponent_rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td class=\"text-right\">{}</td></tr>",
            text::clean(&row.try_get::<String, _>("title")?),
            countries::name(&row.try_get::<String, _>("country")?).unwrap_or(""),
            admin::delete_button(&ctx, &format!("admin/events/opponents/delete/{}", row.try_get::<u32, _>("opponent_id")?), "Supprimer cet adversaire ET ses matchs ?"),
        ));
    }
    let country_options = std::iter::once(("", "— Pays —")).chain(countries::COUNTRIES.iter().copied());
    let opponent_form = format!(
        "<form method=\"post\" action=\"{}\" enctype=\"multipart/form-data\">{}\
         <input type=\"text\" class=\"form-control mb-2\" name=\"title\" maxlength=\"100\" placeholder=\"Nom\" required />\
         <input type=\"text\" class=\"form-control mb-2\" name=\"website\" maxlength=\"100\" placeholder=\"Site web (https://…)\" />{}{}\
         <button type=\"submit\" class=\"btn btn-primary\">Ajouter</button></form>",
        ctx.url("admin/events/opponents/add"),
        form::csrf_field(&ctx),
        form::select("Pays", "country", "", country_options),
        form::image("Logo", "image", None, ""),
    );

    let side = |title: &str, form_html: String, rows: String| {
        format!(
            "<div class=\"card mb-4\"><div class=\"card-body\"><h5>{title}</h5>{form_html}</div>\
             <table class=\"table table-hover mb-0\"><tbody>{rows}</tbody></table></div>"
        )
    };
    let content = format!(
        "<div class=\"row\"><div class=\"col-12 col-xl-8\">{}</div><div class=\"col-12 col-xl-4 mt-4 mt-xl-0\">{}{}</div></div>",
        admin::list_card(&ctx, TITLE, Some("admin/events/add"), &["Titre", "Type", "Date", "État", ""], &body),
        side("Types d'événements", type_form, type_rows),
        side("Adversaires", opponent_form, opponent_rows),
    );
    admin::render(&ctx, "events", TITLE, notice.message(), content)
}

#[derive(Default)]
struct Values {
    title: String,
    type_id: String,
    date: String,
    date_end: String,
    location: String,
    description: String,
    private_description: String,
    published: bool,
    team_id: String,
    opponent_id: String,
    mode_id: String,
    webtv: String,
    website: String,
}

impl Values {
    fn from_data(data: &FormData) -> Self {
        let get = |name: &str| data.text(name).trim().to_owned();
        Self {
            title: get("title"),
            type_id: get("type_id"),
            date: get("date"),
            date_end: get("date_end"),
            location: get("location"),
            description: get("description"),
            private_description: get("private_description"),
            published: data.checked("published"),
            team_id: get("team_id"),
            opponent_id: get("opponent_id"),
            mode_id: get("mode_id"),
            webtv: get("webtv"),
            website: get("website"),
        }
    }
}

fn parse_datetime(raw: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M")
        .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S"))
        .ok()
}

fn datetime_value(date: &NaiveDateTime) -> String {
    date.with_second(0).unwrap_or(*date).format("%Y-%m-%dT%H:%M").to_string()
}

fn datetime_field(label: &str, name: &str, value: &str, required: bool) -> String {
    format!(
        "<div class=\"form-group\"><label for=\"f-{name}\"><b>{label}</b></label>\
         <input type=\"datetime-local\" class=\"form-control\" id=\"f-{name}\" name=\"{name}\" value=\"{}\"{} /></div>",
        text::encode(value),
        if required { " required" } else { "" }
    )
}

/// Liste déroulante alimentée par une requête `(id, libellé)`.
async fn db_select(ctx: &Ctx, label: &str, name: &str, selected: &str, empty: Option<&str>, sql: &str) -> Result<String, AppError> {
    let rows = sqlx::query(sql).fetch_all(&ctx.state.db).await?;
    let mut options: Vec<(String, String)> = empty.map(|e| (String::new(), e.to_owned())).into_iter().collect();
    for row in &rows {
        options.push((row.try_get::<u32, _>(0)?.to_string(), text::decode(&row.try_get::<String, _>(1)?)));
    }
    Ok(form::select(label, name, selected, options.iter().map(|(v, t)| (v.as_str(), t.as_str()))))
}

async fn event_form(ctx: &Ctx, action: &str, values: &Values, event: Option<&Event>, error: Option<&str>) -> Reply {
    let half = |html: String| format!("<div class=\"col-12 col-md-6\">{html}</div>");
    let fields = format!(
        "{}<div class=\"row\">{}{}{}{}</div>{}{}{}{}<legend>Match</legend>\
         <p class=\"text-muted\">Facultatif, pour les types liés aux équipes. Choisissez une équipe et un adversaire pour en faire un match.</p>\
         <div class=\"row\">{}{}{}{}{}</div>",
        input("Titre", "title", &values.title, 100, true, ""),
        half(db_select(ctx, "Type", "type_id", &values.type_id, None, "SELECT type_id, title FROM nf_events_types ORDER BY title").await?),
        half(input("Lieu", "location", &values.location, 200, false, "")),
        half(datetime_field("Début", "date", &values.date, true)),
        half(datetime_field("Fin", "date_end", &values.date_end, false)),
        admin::image_field(ctx, "Image", "image", event.and_then(|e| e.image.as_deref()), ""),
        textarea("Description", "description", &values.description, 8, false),
        textarea("Informations réservées aux participants", "private_description", &values.private_description, 4, false),
        checkbox("Publié", "published", values.published),
        half(teams::select(ctx, "Équipe", "team_id", &values.team_id, Some("— Pas de match —")).await?),
        half(db_select(ctx, "Adversaire", "opponent_id", &values.opponent_id, Some("—"), "SELECT opponent_id, title FROM nf_events_matches_opponents ORDER BY title").await?),
        half(db_select(
            ctx,
            "Mode de jeu",
            "mode_id",
            &values.mode_id,
            Some("—"),
            "SELECT m.mode_id, CONCAT(g.name, ' — ', m.title) FROM nf_games_modes m INNER JOIN nf_games g ON g.game_id = m.game_id ORDER BY g.name, m.title"
        )
        .await?),
        half(input("WebTV", "webtv", &values.webtv, 100, false, "Adresse complète (https://…)")),
        half(input("Site du match", "website", &values.website, 100, false, "Adresse complète (https://…)")),
    );
    let mut content = admin::upload_card(ctx, action, error, fields, "admin/events");

    if let Some(event) = event {
        let id = event.event_id;
        if let Some(game) = match_of(ctx, id).await? {
            let mut rows = String::new();
            for (i, r) in game.rounds.iter().enumerate() {
                rows.push_str(&format!(
                    "<tr><td>Manche {}</td><td>{}</td><td>{} - {}</td><td class=\"text-right\">{}</td></tr>",
                    i + 1,
                    r.map.as_deref().unwrap_or("—"),
                    r.score1,
                    r.score2,
                    admin::delete_button(ctx, &format!("admin/events/rounds/delete/{}", r.round_id), "Supprimer cette manche ?"),
                ));
            }
            let maps = sqlx::query(
                "SELECT gm.map_id, gm.title FROM nf_games_maps gm INNER JOIN nf_teams t ON t.game_id = gm.game_id WHERE t.team_id = ? ORDER BY gm.title",
            )
            .bind(game.team_id)
            .fetch_all(&ctx.state.db)
            .await?;
            let mut map_options = String::from("<option value=\"\">— Carte —</option>");
            for row in &maps {
                map_options.push_str(&format!(
                    "<option value=\"{}\">{}</option>",
                    row.try_get::<u32, _>("map_id")?,
                    text::clean(&row.try_get::<String, _>("title")?)
                ));
            }
            content.push_str(&format!(
                "<div class=\"card mt-4\"><div class=\"card-body\"><h5>Manches et scores — {} vs {}</h5>\
                 <form method=\"post\" action=\"{}\" class=\"form-inline\">{}<select class=\"form-control mr-2\" name=\"map_id\">{map_options}</select>\
                 <input type=\"number\" class=\"form-control mr-2\" name=\"score1\" min=\"0\" max=\"9999\" placeholder=\"Notre score\" required />\
                 <input type=\"number\" class=\"form-control mr-2\" name=\"score2\" min=\"0\" max=\"9999\" placeholder=\"Score adverse\" required />\
                 <button type=\"submit\" class=\"btn btn-primary\">Ajouter</button></form></div>\
                 <table class=\"table table-hover mb-0\"><tbody>{rows}</tbody></table></div>",
                game.team_title,
                game.opponent_title,
                ctx.url(&format!("admin/events/{id}/rounds/add")),
                form::csrf_field(ctx),
            ));
        }

        let mut rows = String::new();
        for p in participants(ctx, id).await? {
            let (label, color, _) = STATUSES[p.status];
            rows.push_str(&format!(
                "<tr><td>{}</td><td><span class=\"badge badge-{color}\">{label}</span></td><td class=\"text-right\">{}</td></tr>",
                text::clean(&p.username),
                admin::delete_button(ctx, &format!("admin/events/{id}/participants/delete/{}", p.user_id), "Retirer ce participant ?"),
            ));
        }
        content.push_str(&format!(
            "<div class=\"card mt-4\"><div class=\"card-body\"><h5>Participants invités</h5>\
             <form method=\"post\" action=\"{}\" class=\"form-inline\">{}\
             <input type=\"text\" class=\"form-control mr-2\" name=\"username\" placeholder=\"Pseudo du membre\" />\
             <button type=\"submit\" class=\"btn btn-primary mr-2\">Inviter</button>\
             <button type=\"submit\" class=\"btn btn-secondary\" name=\"team\" value=\"on\">Inviter toute l'équipe du match</button></form></div>\
             <table class=\"table table-hover mb-0\"><tbody>{rows}</tbody></table></div>",
            ctx.url(&format!("admin/events/{id}/participants/add")),
            form::csrf_field(ctx),
        ));
    }
    admin::render(ctx, "events", if event.is_some() { "Modifier l'événement" } else { "Ajouter un événement" }, None, content)
}

struct Valid {
    type_id: u32,
    date: NaiveDateTime,
    date_end: Option<NaiveDateTime>,
    /// (équipe, adversaire, mode)
    game: Option<(u32, u32, Option<u32>)>,
}

async fn validate(ctx: &Ctx, v: &Values) -> Result<Result<Valid, &'static str>, AppError> {
    let db = &ctx.state.db;
    if v.title.is_empty() || text::url_title(&v.title).is_empty() || !fits(&v.title, 100) {
        return Ok(Err("Le titre est obligatoire (100 caractères maximum)."));
    }
    let kind: Option<i64> = match v.type_id.parse::<u32>() {
        Ok(id) => sqlx::query_scalar("SELECT CAST(type AS SIGNED) FROM nf_events_types WHERE type_id = ?")
            .bind(id)
            .fetch_optional(db)
            .await?,
        Err(_) => None,
    };
    let (Some(kind), Ok(type_id)) = (kind, v.type_id.parse::<u32>()) else {
        return Ok(Err("Type d'événement invalide."));
    };
    let Some(date) = parse_datetime(&v.date) else {
        return Ok(Err("Date de début invalide."));
    };
    let date_end = if v.date_end.is_empty() {
        None
    } else {
        match parse_datetime(&v.date_end) {
            Some(end) if end >= date => Some(end),
            _ => return Ok(Err("La date de fin doit suivre la date de début.")),
        }
    };
    if !valid_url(&v.webtv) || !valid_url(&v.website) {
        return Ok(Err("Les adresses doivent être complètes (https://…)."));
    }

    let game = if v.team_id.is_empty() {
        None
    } else {
        if kind != 1 {
            return Ok(Err("Ce type d'événement n'est pas lié aux équipes : il ne peut pas porter de match."));
        }
        let team_id = v.team_id.parse::<u32>().ok();
        let team_ok = match team_id {
            Some(id) => teams::find(ctx, id).await?.is_some(),
            None => false,
        };
        let opponent_id: Option<u32> = match v.opponent_id.parse::<u32>() {
            Ok(id) => sqlx::query_scalar("SELECT opponent_id FROM nf_events_matches_opponents WHERE opponent_id = ?")
                .bind(id)
                .fetch_optional(db)
                .await?,
            Err(_) => None,
        };
        let mode_id: Option<u32> = match v.mode_id.parse::<u32>() {
            Ok(id) => sqlx::query_scalar("SELECT mode_id FROM nf_games_modes WHERE mode_id = ?")
                .bind(id)
                .fetch_optional(db)
                .await?,
            Err(_) => None,
        };
        match (team_id, team_ok, opponent_id) {
            (Some(team_id), true, Some(opponent_id)) => Some((team_id, opponent_id, mode_id)),
            _ => return Ok(Err("Pour un match, choisissez une équipe et un adversaire.")),
        }
    };
    Ok(Ok(Valid { type_id, date, date_end, game }))
}

async fn save(ctx: &Ctx, multipart: Multipart, event: Option<Event>) -> Reply {
    let mut data = admin::read_upload_form(ctx, multipart).await?;
    let values = Values::from_data(&data);
    let action = match &event {
        Some(event) => format!("admin/{}", event.path()),
        None => "admin/events/add".to_owned(),
    };
    let valid = match validate(ctx, &values).await? {
        Ok(valid) => valid,
        Err(message) => return event_form(ctx, &action, &values, event.as_ref(), Some(message)).await,
    };
    let Some(author) = &ctx.user else {
        return layout::forbidden(ctx).await;
    };
    let old_image = event.as_ref().and_then(|e| e.image_id);
    let image = match admin::image_change(ctx, &mut data, "image", "events", Fit::MaxWidth(1600), old_image).await? {
        Ok(change) => change,
        Err(message) => return event_form(ctx, &action, &values, event.as_ref(), Some(message)).await,
    };

    let mut tx = ctx.state.db.begin().await?;
    let published = if values.published { "1" } else { "0" };
    let event_id = match &event {
        Some(event) => {
            sqlx::query(
                "UPDATE nf_events SET type_id = ?, image_id = ?, title = ?, description = ?, private_description = ?, \
                 location = ?, date = ?, date_end = ?, published = ? WHERE event_id = ?",
            )
            .bind(valid.type_id)
            .bind(image.value)
            .bind(text::encode(&values.title))
            .bind(text::encode(&values.description))
            .bind(text::encode(&values.private_description))
            .bind(text::encode(&values.location))
            .bind(valid.date)
            .bind(valid.date_end)
            .bind(published)
            .bind(event.event_id)
            .execute(&mut *tx)
            .await?;
            event.event_id as u64
        }
        None => sqlx::query(
            "INSERT INTO nf_events (type_id, user_id, image_id, title, description, private_description, location, date, date_end, published) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(valid.type_id)
        .bind(author.id)
        .bind(image.value)
        .bind(text::encode(&values.title))
        .bind(text::encode(&values.description))
        .bind(text::encode(&values.private_description))
        .bind(text::encode(&values.location))
        .bind(valid.date)
        .bind(valid.date_end)
        .bind(published)
        .execute(&mut *tx)
        .await?
        .last_insert_id(),
    };
    match valid.game {
        Some((team_id, opponent_id, mode_id)) => {
            sqlx::query(
                "INSERT INTO nf_events_matches (event_id, team_id, opponent_id, mode_id, webtv, website) VALUES (?, ?, ?, ?, ?, ?) \
                 ON DUPLICATE KEY UPDATE team_id = VALUES(team_id), opponent_id = VALUES(opponent_id), mode_id = VALUES(mode_id), \
                    webtv = VALUES(webtv), website = VALUES(website)",
            )
            .bind(event_id)
            .bind(team_id)
            .bind(opponent_id)
            .bind(mode_id)
            .bind(text::encode(&values.webtv))
            .bind(text::encode(&values.website))
            .execute(&mut *tx)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM nf_events_matches_rounds WHERE event_id = ?").bind(event_id).execute(&mut *tx).await?;
            sqlx::query("DELETE FROM nf_events_matches WHERE event_id = ?").bind(event_id).execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;
    image.finish(ctx).await?;
    if event.is_some() {
        admin::redirect(ctx, "admin/events?ok=1")
    } else {
        // Sur la fiche : manches et participants se gèrent une fois l'événement créé.
        admin::redirect(ctx, &format!("admin/events/{event_id}/{}", text::url_title(&values.title)))
    }
}

pub async fn admin_add_get(Extension(ctx): Ctxt) -> Reply {
    admin_guard!(ctx, "events");
    let values = Values { published: true, ..Values::default() };
    event_form(&ctx, "admin/events/add", &values, None, None).await
}

pub async fn admin_add_post(Extension(ctx): Ctxt, multipart: Multipart) -> Reply {
    admin_guard!(ctx, "events");
    save(&ctx, multipart, None).await
}

pub async fn admin_edit_get(Extension(ctx): Ctxt, Path((id, _slug)): Path<(u32, String)>) -> Reply {
    admin_guard!(ctx, "events");
    let Some(event) = find(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let game = match_of(&ctx, id).await?;
    let values = Values {
        title: text::decode(&event.title),
        type_id: event.type_id.to_string(),
        date: datetime_value(&event.date),
        date_end: event.date_end.as_ref().map(datetime_value).unwrap_or_default(),
        location: text::decode(&event.location),
        description: text::decode(&event.description),
        private_description: text::decode(&event.private_description),
        published: event.published,
        team_id: game.as_ref().map(|g| g.team_id.to_string()).unwrap_or_default(),
        opponent_id: game.as_ref().map(|g| g.opponent_id.to_string()).unwrap_or_default(),
        mode_id: game.as_ref().and_then(|g| g.mode_id).map(|m| m.to_string()).unwrap_or_default(),
        webtv: game.as_ref().map(|g| text::decode(&g.webtv)).unwrap_or_default(),
        website: game.as_ref().map(|g| text::decode(&g.website)).unwrap_or_default(),
    };
    event_form(&ctx, &format!("admin/{}", event.path()), &values, Some(&event), None).await
}

pub async fn admin_edit_post(Extension(ctx): Ctxt, Path((id, _slug)): Path<(u32, String)>, multipart: Multipart) -> Reply {
    admin_guard!(ctx, "events");
    let Some(event) = find(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    save(&ctx, multipart, Some(event)).await
}

pub async fn admin_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx, "events");
    admin::check_csrf(&ctx, &form.csrf)?;
    let image = find(&ctx, id).await?.and_then(|e| e.image_id);
    let mut tx = ctx.state.db.begin().await?;
    sqlx::query("DELETE FROM nf_comment WHERE module = 'events' AND module_id = ?").bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM nf_events WHERE event_id = ?").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    if let Some(image) = image {
        crate::core::files::delete(&ctx, image).await?;
    }
    admin::redirect(&ctx, "admin/events?ok=1")
}

async fn back_to_event(ctx: &Ctx, event_id: u32) -> Reply {
    admin::redirect(ctx, &format!("admin/events/{event_id}/x"))
}

#[derive(Deserialize)]
pub struct RoundForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    map_id: String,
    #[serde(default)]
    score1: u16,
    #[serde(default)]
    score2: u16,
}

pub async fn admin_round_add(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<RoundForm>) -> Reply {
    admin_guard!(ctx, "events");
    admin::check_csrf(&ctx, &form.csrf)?;
    let db = &ctx.state.db;
    if match_of(&ctx, id).await?.is_none() {
        return layout::not_found(&ctx).await;
    }
    let map_id: Option<u32> = match form.map_id.parse::<u32>() {
        Ok(map_id) => sqlx::query_scalar("SELECT map_id FROM nf_games_maps WHERE map_id = ?")
            .bind(map_id)
            .fetch_optional(db)
            .await?,
        Err(_) => None,
    };
    sqlx::query("INSERT INTO nf_events_matches_rounds (event_id, map_id, score1, score2) VALUES (?, ?, ?, ?)")
        .bind(id)
        .bind(map_id)
        .bind(form.score1)
        .bind(form.score2)
        .execute(db)
        .await?;
    back_to_event(&ctx, id).await
}

pub async fn admin_round_delete(Extension(ctx): Ctxt, Path(round_id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx, "events");
    admin::check_csrf(&ctx, &form.csrf)?;
    let db = &ctx.state.db;
    let event_id: Option<u32> = sqlx::query_scalar("SELECT event_id FROM nf_events_matches_rounds WHERE round_id = ?")
        .bind(round_id)
        .fetch_optional(db)
        .await?;
    sqlx::query("DELETE FROM nf_events_matches_rounds WHERE round_id = ?").bind(round_id).execute(db).await?;
    match event_id {
        Some(event_id) => back_to_event(&ctx, event_id).await,
        None => admin::redirect(&ctx, "admin/events"),
    }
}

#[derive(Deserialize)]
pub struct InviteForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    username: String,
    team: Option<String>,
}

pub async fn admin_participant_add(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<InviteForm>) -> Reply {
    admin_guard!(ctx, "events");
    admin::check_csrf(&ctx, &form.csrf)?;
    let db = &ctx.state.db;
    if find(&ctx, id).await?.is_none() {
        return layout::not_found(&ctx).await;
    }
    if form.team.is_some() {
        // INSERT IGNORE : ceux qui ont déjà répondu gardent leur réponse.
        sqlx::query(
            "INSERT IGNORE INTO nf_events_participants (event_id, user_id, status) \
             SELECT m.event_id, tu.user_id, 0 FROM nf_events_matches m \
             INNER JOIN nf_teams_users tu ON tu.team_id = m.team_id \
             INNER JOIN nf_user u ON u.id = tu.user_id AND u.deleted = '0' WHERE m.event_id = ?",
        )
        .bind(id)
        .execute(db)
        .await?;
    } else {
        let user_id: Option<u32> = sqlx::query_scalar("SELECT id FROM nf_user WHERE username = ? AND deleted = '0'")
            .bind(text::encode(form.username.trim()))
            .fetch_optional(db)
            .await?;
        let Some(user_id) = user_id else {
            return Err(AppError::BadRequest("Aucun membre ne porte ce pseudo."));
        };
        sqlx::query("INSERT IGNORE INTO nf_events_participants (event_id, user_id, status) VALUES (?, ?, 0)")
            .bind(id)
            .bind(user_id)
            .execute(db)
            .await?;
    }
    back_to_event(&ctx, id).await
}

pub async fn admin_participant_delete(Extension(ctx): Ctxt, Path((id, user_id)): Path<(u32, u32)>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx, "events");
    admin::check_csrf(&ctx, &form.csrf)?;
    sqlx::query("DELETE FROM nf_events_participants WHERE event_id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(&ctx.state.db)
        .await?;
    back_to_event(&ctx, id).await
}

#[derive(Deserialize)]
pub struct TypeForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    icon: String,
    #[serde(default)]
    color: String,
    team: Option<String>,
}

pub async fn admin_type_add(Extension(ctx): Ctxt, Form(form): Form<TypeForm>) -> Reply {
    admin_guard!(ctx, "events");
    admin::check_csrf(&ctx, &form.csrf)?;
    let title = form.title.trim();
    let icon = form.icon.trim();
    if title.is_empty() || !fits(title, 100) {
        return Err(AppError::BadRequest("Le titre est obligatoire (100 caractères maximum)."));
    }
    let icon = if text::icon(icon).is_empty() || icon.len() > 50 { "far fa-calendar-alt" } else { icon };
    let color = COLORS.iter().find(|c| **c == form.color).copied().unwrap_or("default");
    let mut tx = ctx.state.db.begin().await?;
    let created = sqlx::query("INSERT INTO nf_events_types (type, title, color, icon) VALUES (?, ?, ?, ?)")
        .bind(if form.team.is_some() { 1 } else { 0 })
        .bind(text::encode(title))
        .bind(color)
        .bind(icon)
        .execute(&mut *tx)
        .await?;
    // Visible par tous par défaut, comme les types d'origine.
    sqlx::query("INSERT IGNORE INTO nf_access (id, module, action) VALUES (?, 'events', 'access_events_type')")
        .bind(created.last_insert_id())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    admin::redirect(&ctx, "admin/events")
}

pub async fn admin_type_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx, "events");
    admin::check_csrf(&ctx, &form.csrf)?;
    let mut tx = ctx.state.db.begin().await?;
    sqlx::query("DELETE FROM nf_access WHERE module = 'events' AND action = 'access_events_type' AND id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM nf_events_types WHERE type_id = ?").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    admin::redirect(&ctx, "admin/events")
}

pub async fn admin_opponent_add(Extension(ctx): Ctxt, multipart: Multipart) -> Reply {
    admin_guard!(ctx, "events");
    let mut data = admin::read_upload_form(&ctx, multipart).await?;
    let title = data.text("title").trim().to_owned();
    let website = data.text("website").trim().to_owned();
    let country = data.text("country").to_owned();
    if title.is_empty() || !fits(&title, 100) {
        return Err(AppError::BadRequest("Le nom est obligatoire (100 caractères maximum)."));
    }
    if !valid_url(&website) {
        return Err(AppError::BadRequest("Le site web doit être une adresse complète (https://…)."));
    }
    if !country.is_empty() && countries::name(&country).is_none() {
        return Err(AppError::BadRequest("Pays invalide."));
    }
    let image = admin::image_change(&ctx, &mut data, "image", "events/opponents", Fit::Square(200), None)
        .await?
        .map_err(AppError::BadRequest)?;
    sqlx::query("INSERT INTO nf_events_matches_opponents (image_id, title, website, country) VALUES (?, ?, ?, ?)")
        .bind(image.value)
        .bind(text::encode(&title))
        .bind(text::encode(&website))
        .bind(country)
        .execute(&ctx.state.db)
        .await?;
    admin::redirect(&ctx, "admin/events")
}

pub async fn admin_opponent_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx, "events");
    admin::check_csrf(&ctx, &form.csrf)?;
    let db = &ctx.state.db;
    let image: Option<u32> = sqlx::query_scalar("SELECT image_id FROM nf_events_matches_opponents WHERE opponent_id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?
        .flatten();
    sqlx::query("DELETE FROM nf_events_matches_opponents WHERE opponent_id = ?").bind(id).execute(db).await?;
    if let Some(image) = image {
        crate::core::files::delete(&ctx, image).await?;
    }
    admin::redirect(&ctx, "admin/events")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(rounds: &[(u32, u32)]) -> Match {
        Match {
            team_id: 1,
            team_title: String::new(),
            team_name: String::new(),
            opponent_id: 1,
            opponent_title: String::new(),
            opponent_image: None,
            opponent_website: String::new(),
            opponent_country: String::new(),
            mode_id: None,
            mode: None,
            webtv: String::new(),
            website: String::new(),
            rounds: rounds.iter().map(|(a, b)| Round { round_id: 0, map: None, score1: *a, score2: *b }).collect(),
        }
    }

    #[test]
    fn global_score() {
        assert_eq!(game(&[]).score(), None);
        assert_eq!(game(&[(16, 12)]).score(), Some((16, 12)));
        assert_eq!(game(&[(16, 12), (9, 16), (16, 14)]).score(), Some((2, 1)));
        assert_eq!(game(&[(15, 15), (16, 3)]).score(), Some((1, 0)));
    }

    #[test]
    fn dates_and_urls() {
        assert!(parse_datetime("2026-11-20T20:30").is_some());
        assert!(parse_datetime("20/11/2026").is_none());
        assert!(valid_url(""));
        assert!(valid_url("https://twitch.tv/neofrag"));
        assert!(!valid_url("javascript:alert(1)"));
        assert!(!valid_url("https://a.fr/\" onclick=\"x"));
        assert_eq!(external_link("javascript:alert(1)", "x", ""), "");
    }
}
