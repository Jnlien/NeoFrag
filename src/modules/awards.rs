//! Module Palmarès. Porté de `modules/awards` et `widgets/awards`.

use super::{
    admin::{self, admin_guard, Ctxt, CsrfOnly, Notice, Reply},
    games, teams,
};
use crate::{
    core::{
        bbcode,
        ctx::Ctx,
        files::{Fit, FormData},
        form::{self, fits, input, textarea},
        layout::{self, Page},
        panel::{Block, Panel},
        text,
    },
    error::AppError,
};
use axum::{
    extract::{Multipart, Path, Query},
    Extension, Form,
};
use chrono::{Datelike, NaiveDate};
use sqlx::Row;

pub const TITLE: &str = "Palmarès";
pub const ICON: &str = "fas fa-trophy";

struct Award {
    award_id: u32,
    team_id: Option<u32>,
    game_id: u32,
    name: String,
    location: String,
    date: NaiveDate,
    description: String,
    platform: String,
    ranking: i64,
    participants: i64,
    image: Option<String>,
    image_id: Option<u32>,
    team_title: Option<String>,
    team_name: Option<String>,
    game_title: String,
    game_name: String,
}

impl Award {
    fn path(&self) -> String {
        format!("awards/{}/{}", self.award_id, text::url_title(&self.name))
    }

    /// Classement, avec une coupe colorée pour le podium.
    fn ranking_html(&self) -> String {
        let color = match self.ranking {
            1 => "#ffd700",
            2 => "#c0c0c0",
            3 => "#cd7f32",
            _ => return format!("{}<sup>e</sup>", self.ranking),
        };
        format!(
            "<span style=\"color: {color}\">{}</span> {}<sup>{}</sup>",
            text::icon("fas fa-trophy"),
            self.ranking,
            if self.ranking == 1 { "er" } else { "e" }
        )
    }
}

const SELECT: &str = "SELECT a.award_id, a.team_id, a.game_id, a.name, a.location, a.date, a.description, a.platform, \
        CAST(a.ranking AS SIGNED) AS ranking, CAST(a.participants AS SIGNED) AS participants, a.image_id, f.path AS image, tl.title AS team_title, t.name AS team_name, \
        gl.title AS game_title, g.name AS game_name \
     FROM nf_awards a \
     INNER JOIN nf_games g ON g.game_id = a.game_id \
     INNER JOIN nf_games_lang gl ON gl.game_id = g.game_id AND gl.lang = ? \
     LEFT JOIN nf_teams t ON t.team_id = a.team_id \
     LEFT JOIN nf_teams_lang tl ON tl.team_id = t.team_id AND tl.lang = ? \
     LEFT JOIN nf_file f ON f.id = a.image_id";

fn from_row(row: &sqlx::mysql::MySqlRow) -> sqlx::Result<Award> {
    let clean = |name: &str| row.try_get::<String, _>(name).map(|v| text::clean(&v));
    Ok(Award {
        award_id: row.try_get("award_id")?,
        team_id: row.try_get("team_id")?,
        game_id: row.try_get("game_id")?,
        name: clean("name")?,
        location: clean("location")?,
        date: row.try_get("date")?,
        description: clean("description")?,
        platform: clean("platform")?,
        ranking: row.try_get("ranking")?,
        participants: row.try_get("participants")?,
        image: row.try_get("image")?,
        image_id: row.try_get("image_id")?,
        team_title: row.try_get::<Option<String>, _>("team_title")?.map(|t| text::clean(&t)),
        team_name: row.try_get("team_name")?,
        game_title: clean("game_title")?,
        game_name: row.try_get("game_name")?,
    })
}

enum Filter {
    All,
    Team(u32),
    Game(u32),
}

async fn list(ctx: &Ctx, filter: &Filter, limit: u32) -> Result<Vec<Award>, AppError> {
    let condition = match filter {
        Filter::All => "",
        Filter::Team(_) => " WHERE a.team_id = ?",
        Filter::Game(_) => " WHERE a.game_id = ?",
    };
    let sql = format!("{SELECT}{condition} ORDER BY a.date DESC, a.award_id DESC LIMIT ?");
    let mut query = sqlx::query(&sql).bind(ctx.lang()).bind(ctx.lang());
    if let Filter::Team(id) | Filter::Game(id) = filter {
        query = query.bind(*id);
    }
    let rows = query.bind(limit).fetch_all(&ctx.state.db).await?;
    Ok(rows.iter().map(from_row).collect::<sqlx::Result<_>>()?)
}

async fn find(ctx: &Ctx, id: u32) -> Result<Option<Award>, AppError> {
    let row = sqlx::query(&format!("{SELECT} WHERE a.award_id = ?"))
        .bind(ctx.lang())
        .bind(ctx.lang())
        .bind(id)
        .fetch_optional(&ctx.state.db)
        .await?;
    Ok(row.as_ref().map(from_row).transpose()?)
}

fn date_fr(date: &NaiveDate) -> String {
    format!("{:02}/{:02}/{}", date.day(), date.month(), date.year())
}

fn team_link(ctx: &Ctx, award: &Award) -> String {
    match (award.team_id, &award.team_title) {
        (Some(id), Some(title)) => format!(
            "<a href=\"{}\">{title}</a>",
            ctx.url(&format!("awards/team/{id}/{}", award.team_name.as_deref().unwrap_or("")))
        ),
        _ => String::new(),
    }
}

fn game_link(ctx: &Ctx, award: &Award) -> String {
    format!(
        "<a href=\"{}\">{}</a>",
        ctx.url(&format!("awards/game/{}/{}", award.game_id, award.game_name)),
        award.game_title
    )
}

async fn listing(ctx: &Ctx, filter: Filter, heading: String) -> Reply {
    let awards = list(ctx, &filter, 500).await?;
    let panel = Panel::new().heading(heading, ICON);
    let panel = if awards.is_empty() {
        panel.style("card-info").body("<div class=\"text-center\">Aucun palmarès pour le moment</div>")
    } else {
        let rows: String = awards
            .iter()
            .map(|a| {
                format!(
                    "<tr><td>{}</td><td>{}</td><td><a href=\"{}\"><b>{}</b></a></td><td>{}</td><td>{}</td><td class=\"text-nowrap\">{}</td></tr>",
                    date_fr(&a.date),
                    team_link(ctx, a),
                    ctx.url(&a.path()),
                    a.name,
                    a.location,
                    game_link(ctx, a),
                    a.ranking_html(),
                )
            })
            .collect();
        panel.body_raw(format!(
            "<div class=\"table-responsive\"><table class=\"table table-hover mb-0\"><thead><tr><th>Date</th><th>Équipe</th>\
             <th>Compétition</th><th>Lieu</th><th>Jeu</th><th>Classement</th></tr></thead><tbody>{rows}</tbody></table></div>"
        ))
    };
    layout::render(ctx, Page::new("awards", TITLE, ICON).blocks(&[Block::Panel(panel)])).await
}

pub async fn index(Extension(ctx): Ctxt) -> Reply {
    if !ctx.enabled("awards") {
        return layout::not_found(&ctx).await;
    }
    listing(&ctx, Filter::All, TITLE.to_owned()).await
}

/// `awards/{id}/{nom}` : détail d'une récompense.
pub async fn show(Extension(ctx): Ctxt, Path((id, slug)): Path<(u32, String)>) -> Reply {
    if !ctx.enabled("awards") {
        return layout::not_found(&ctx).await;
    }
    let award = match find(&ctx, id).await? {
        Some(award) if text::url_title(&award.name) == slug => award,
        _ => return layout::not_found(&ctx).await,
    };
    let image = match &award.image {
        Some(path) => format!("<img class=\"card-img-top\" src=\"{}\" alt=\"\" />", text::encode(&ctx.url(path))),
        None => String::new(),
    };
    let mut details = String::new();
    let mut detail = |icon: &str, label: &str, value: String| {
        if !value.is_empty() {
            details.push_str(&format!(
                "<li class=\"list-group-item\">{} <b>{label}</b> <span class=\"float-right\">{value}</span></li>",
                text::icon(icon)
            ));
        }
    };
    detail("fas fa-trophy", "Classement", award.ranking_html());
    detail("far fa-calendar-alt", "Date", date_fr(&award.date));
    detail("fas fa-headset", "Équipe", team_link(&ctx, &award));
    detail("fas fa-gamepad", "Jeu", game_link(&ctx, &award));
    detail("fas fa-map-marker-alt", "Lieu", award.location.clone());
    detail("fas fa-desktop", "Plateforme", award.platform.clone());
    if award.participants > 0 {
        detail("fas fa-users", "Participants", award.participants.to_string());
    }
    let description = if award.description.is_empty() {
        String::new()
    } else {
        format!("<div class=\"card-body border-top\">{}</div>", bbcode::render(&award.description))
    };
    let panel = Panel::new().body_raw(format!(
        "{image}<div class=\"card-body\"><h4 class=\"card-title mb-0\">{}</h4></div><ul class=\"list-group list-group-flush\">{details}</ul>{description}",
        award.name
    ));
    let page = Page::new("awards", TITLE, ICON)
        .title(award.name.clone())
        .crumb(award.name.clone(), award.path())
        .blocks(&[Block::Panel(panel)]);
    layout::render(&ctx, page).await
}

/// `awards/team/{id}/{nom}` et `awards/game/{id}/{nom}`.
pub async fn filtered(Extension(ctx): Ctxt, Path((kind, id, _slug)): Path<(String, u32, String)>) -> Reply {
    if !ctx.enabled("awards") {
        return layout::not_found(&ctx).await;
    }
    match kind.as_str() {
        "team" => match teams::find(&ctx, id).await? {
            Some(team) => listing(&ctx, Filter::Team(id), format!("{TITLE} — {}", team.title)).await,
            None => layout::not_found(&ctx).await,
        },
        "game" => {
            let title: Option<String> =
                sqlx::query_scalar("SELECT title FROM nf_games_lang WHERE game_id = ? AND lang = ?")
                    .bind(id)
                    .bind(ctx.lang())
                    .fetch_optional(&ctx.state.db)
                    .await?;
            match title {
                Some(title) => listing(&ctx, Filter::Game(id), format!("{TITLE} — {}", text::clean(&title))).await,
                None => layout::not_found(&ctx).await,
            }
        }
        _ => layout::not_found(&ctx).await,
    }
}

/// Widget : dernières récompenses.
pub async fn widget(ctx: &Ctx) -> Result<Vec<Block>, AppError> {
    let awards = list(ctx, &Filter::All, 5).await?;
    let panel = Panel::new().heading(TITLE, ICON);
    if awards.is_empty() {
        return Ok(vec![Block::Panel(panel.body("Aucun palmarès pour le moment"))]);
    }
    let items: String = awards
        .iter()
        .map(|a| {
            format!(
                "<li class=\"list-group-item\"><span class=\"float-right\">{}</span><a href=\"{}\">{}</a><br /><small class=\"text-muted\">{} — {}</small></li>",
                a.ranking_html(),
                ctx.url(&a.path()),
                a.name,
                date_fr(&a.date),
                a.game_title
            )
        })
        .collect();
    let footer = format!("<a href=\"{}\">{} Voir tout le palmarès</a>", ctx.url("awards"), text::icon("far fa-arrow-alt-circle-right"));
    Ok(vec![Block::Panel(
        panel
            .body_raw(format!("<ul class=\"list-group list-group-flush\">{items}</ul>"))
            .footer(footer, true),
    )])
}

// ---------------------------------------------------------------------------
// Administration

pub async fn admin_list(Extension(ctx): Ctxt, Query(notice): Query<Notice>) -> Reply {
    admin_guard!(ctx, "awards");
    let mut body = String::new();
    for a in list(&ctx, &Filter::All, 1000).await? {
        body.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td>{}</tr>",
            date_fr(&a.date),
            a.name,
            a.team_title.as_deref().unwrap_or("—"),
            a.game_title,
            a.ranking_html(),
            admin::row_actions(&ctx, &format!("admin/{}", a.path()), &format!("admin/awards/delete/{}", a.award_id), "Supprimer cette récompense ?"),
        ));
    }
    let content = admin::list_card(&ctx, TITLE, Some("admin/awards/add"), &["Date", "Compétition", "Équipe", "Jeu", "Classement", ""], &body);
    admin::render(&ctx, "awards", TITLE, notice.message(), content)
}

#[derive(Default)]
struct Values {
    name: String,
    team_id: String,
    game_id: String,
    date: String,
    location: String,
    platform: String,
    ranking: String,
    participants: String,
    description: String,
}

impl Values {
    fn from_data(data: &FormData) -> Self {
        let get = |name: &str| data.text(name).trim().to_owned();
        Self {
            name: get("name"),
            team_id: get("team_id"),
            game_id: get("game_id"),
            date: get("date"),
            location: get("location"),
            platform: get("platform"),
            ranking: get("ranking"),
            participants: get("participants"),
            description: get("description"),
        }
    }
}

struct Valid {
    team_id: Option<u32>,
    game_id: u32,
    date: NaiveDate,
    ranking: u32,
    participants: u32,
}

async fn validate(ctx: &Ctx, v: &Values) -> Result<Result<Valid, &'static str>, AppError> {
    if v.name.is_empty() || text::url_title(&v.name).is_empty() || !fits(&v.name, 100) {
        return Ok(Err("Le nom de la compétition est obligatoire (100 caractères maximum)."));
    }
    if !fits(&v.location, 100) || !fits(&v.platform, 100) {
        return Ok(Err("Le lieu et la plateforme sont limités à 100 caractères."));
    }
    let game_id = match v.game_id.parse::<u32>() {
        Ok(id) if games::exists(ctx, id).await? => id,
        _ => return Ok(Err("Jeu invalide.")),
    };
    let team_id = if v.team_id.is_empty() {
        None
    } else {
        match v.team_id.parse::<u32>() {
            Ok(id) if teams::find(ctx, id).await?.is_some() => Some(id),
            _ => return Ok(Err("Équipe invalide.")),
        }
    };
    let Ok(date) = NaiveDate::parse_from_str(&v.date, "%Y-%m-%d") else {
        return Ok(Err("Date invalide."));
    };
    let ranking = match v.ranking.parse::<u32>() {
        Ok(n) if (1..=100_000).contains(&n) => n,
        _ => return Ok(Err("Le classement doit être un nombre supérieur à zéro.")),
    };
    let participants = if v.participants.is_empty() {
        0
    } else {
        match v.participants.parse::<u32>() {
            Ok(n) if n <= 1_000_000 => n,
            _ => return Ok(Err("Nombre de participants invalide.")),
        }
    };
    Ok(Ok(Valid { team_id, game_id, date, ranking, participants }))
}

async fn award_form(ctx: &Ctx, action: &str, v: &Values, award: Option<&Award>, error: Option<&str>) -> Reply {
    if games::all(ctx).await?.is_empty() {
        let content = format!("<div class=\"alert alert-info\">Créez d'abord un <a href=\"{}\">jeu</a>.</div>", ctx.url("admin/games/add"));
        return admin::render(ctx, "awards", TITLE, None, content);
    }
    let half = |html: String| format!("<div class=\"col-12 col-md-6\">{html}</div>");
    let number = |label: &str, name: &str, value: &str, required: bool| {
        format!(
            "<div class=\"form-group\"><label for=\"f-{name}\"><b>{label}</b></label>\
             <input type=\"number\" class=\"form-control\" id=\"f-{name}\" name=\"{name}\" value=\"{}\" min=\"0\"{} /></div>",
            text::encode(value),
            if required { " required" } else { "" }
        )
    };
    let fields = format!(
        "{}<div class=\"row\">{}{}{}{}{}{}{}</div>{}{}",
        input("Compétition", "name", &v.name, 100, true, ""),
        half(games::select(ctx, "Jeu", "game_id", &v.game_id, None).await?),
        half(teams::select(ctx, "Équipe", "team_id", &v.team_id, Some("—")).await?),
        half(form::date("Date", "date", &v.date)),
        half(input("Lieu", "location", &v.location, 100, false, "")),
        half(input("Plateforme", "platform", &v.platform, 100, false, "")),
        half(number("Classement", "ranking", &v.ranking, true)),
        half(number("Participants", "participants", &v.participants, false)),
        admin::image_field(ctx, "Image", "image", award.and_then(|a| a.image.as_deref()), ""),
        textarea("Description", "description", &v.description, 6, false),
    );
    let title = if award.is_some() { "Modifier la récompense" } else { "Ajouter une récompense" };
    admin::render(ctx, "awards", title, None, admin::upload_card(ctx, action, error, fields, "admin/awards"))
}

async fn save(ctx: &Ctx, multipart: Multipart, award: Option<Award>) -> Reply {
    let mut data = admin::read_upload_form(ctx, multipart).await?;
    let v = Values::from_data(&data);
    let action = match &award {
        Some(a) => format!("admin/{}", a.path()),
        None => "admin/awards/add".to_owned(),
    };
    let valid = match validate(ctx, &v).await? {
        Ok(valid) => valid,
        Err(message) => return award_form(ctx, &action, &v, award.as_ref(), Some(message)).await,
    };
    let old = award.as_ref().and_then(|a| a.image_id);
    let image = match admin::image_change(ctx, &mut data, "image", "awards", Fit::MaxWidth(1600), old).await? {
        Ok(change) => change,
        Err(message) => return award_form(ctx, &action, &v, award.as_ref(), Some(message)).await,
    };
    let sql = match &award {
        Some(_) => {
            "UPDATE nf_awards SET team_id = ?, game_id = ?, image_id = ?, name = ?, location = ?, date = ?, description = ?, \
             platform = ?, ranking = ?, participants = ? WHERE award_id = ?"
        }
        None => {
            "INSERT INTO nf_awards (team_id, game_id, image_id, name, location, date, description, platform, ranking, participants) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        }
    };
    let mut query = sqlx::query(sql)
        .bind(valid.team_id)
        .bind(valid.game_id)
        .bind(image.value)
        .bind(text::encode(&v.name))
        .bind(text::encode(&v.location))
        .bind(valid.date)
        .bind(text::encode(&v.description))
        .bind(text::encode(&v.platform))
        .bind(valid.ranking)
        .bind(valid.participants);
    if let Some(a) = &award {
        query = query.bind(a.award_id);
    }
    query.execute(&ctx.state.db).await?;
    // L'ancienne image n'est supprimée qu'après la mise à jour (clé étrangère en cascade).
    image.finish(ctx).await?;
    admin::redirect(ctx, "admin/awards?ok=1")
}

pub async fn admin_add_get(Extension(ctx): Ctxt) -> Reply {
    admin_guard!(ctx, "awards");
    award_form(&ctx, "admin/awards/add", &Values::default(), None, None).await
}

pub async fn admin_add_post(Extension(ctx): Ctxt, multipart: Multipart) -> Reply {
    admin_guard!(ctx, "awards");
    save(&ctx, multipart, None).await
}

pub async fn admin_edit_get(Extension(ctx): Ctxt, Path((id, _slug)): Path<(u32, String)>) -> Reply {
    admin_guard!(ctx, "awards");
    let Some(a) = find(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let v = Values {
        name: text::decode(&a.name),
        team_id: a.team_id.map(|t| t.to_string()).unwrap_or_default(),
        game_id: a.game_id.to_string(),
        date: a.date.format("%Y-%m-%d").to_string(),
        location: text::decode(&a.location),
        platform: text::decode(&a.platform),
        ranking: a.ranking.to_string(),
        participants: a.participants.to_string(),
        description: text::decode(&a.description),
    };
    award_form(&ctx, &format!("admin/{}", a.path()), &v, Some(&a), None).await
}

pub async fn admin_edit_post(Extension(ctx): Ctxt, Path((id, _slug)): Path<(u32, String)>, multipart: Multipart) -> Reply {
    admin_guard!(ctx, "awards");
    let Some(a) = find(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    save(&ctx, multipart, Some(a)).await
}

pub async fn admin_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx, "awards");
    admin::check_csrf(&ctx, &form.csrf)?;
    let image = find(&ctx, id).await?.and_then(|a| a.image_id);
    sqlx::query("DELETE FROM nf_awards WHERE award_id = ?")
        .bind(id)
        .execute(&ctx.state.db)
        .await?;
    if let Some(image) = image {
        crate::core::files::delete(&ctx, image).await?;
    }
    admin::redirect(&ctx, "admin/awards?ok=1")
}
