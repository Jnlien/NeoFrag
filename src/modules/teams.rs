//! Module Équipes : équipes, joueurs et rôles. Porté de `modules/teams`.

use super::{
    admin::{self, admin_guard, Ctxt, CsrfOnly, Notice, Reply},
    games,
};
use crate::{
    core::{
        bbcode,
        ctx::Ctx,
        files::Fit,
        form::{self, fits, input, textarea},
        layout::{self, Page},
        panel::{Block, Panel},
        text, user,
    },
    error::AppError,
};
use axum::{
    extract::{Multipart, Path, Query},
    Extension, Form,
};
use serde::Deserialize;
use sqlx::Row;

pub const TITLE: &str = "Équipes";
pub const ICON: &str = "fas fa-headset";

pub struct Team {
    pub team_id: u32,
    pub game_id: u32,
    pub name: String,
    pub title: String,
    pub description: String,
    pub image: Option<String>,
    pub icon: Option<String>,
    pub game_title: String,
    pub players: i64,
    image_id: Option<u32>,
    icon_id: Option<u32>,
}

impl Team {
    pub fn path(&self) -> String {
        format!("teams/{}/{}", self.team_id, self.name)
    }
}

const SELECT: &str = "SELECT t.team_id, t.game_id, t.name, tl.title, tl.description, t.image_id, t.icon_id, \
        fi.path AS image, fc.path AS icon, gl.title AS game_title, \
        (SELECT COUNT(*) FROM nf_teams_users tu INNER JOIN nf_user u ON u.id = tu.user_id AND u.deleted = '0' \
          WHERE tu.team_id = t.team_id) AS players \
     FROM nf_teams t \
     INNER JOIN nf_teams_lang tl ON tl.team_id = t.team_id AND tl.lang = ? \
     INNER JOIN nf_games_lang gl ON gl.game_id = t.game_id AND gl.lang = ? \
     LEFT JOIN nf_file fi ON fi.id = t.image_id LEFT JOIN nf_file fc ON fc.id = t.icon_id";

fn from_row(row: &sqlx::mysql::MySqlRow) -> sqlx::Result<Team> {
    Ok(Team {
        team_id: row.try_get("team_id")?,
        game_id: row.try_get("game_id")?,
        name: row.try_get("name")?,
        title: text::clean(&row.try_get::<String, _>("title")?),
        description: text::clean(&row.try_get::<String, _>("description")?),
        image: row.try_get("image")?,
        icon: row.try_get("icon")?,
        game_title: text::clean(&row.try_get::<String, _>("game_title")?),
        players: row.try_get("players")?,
        image_id: row.try_get("image_id")?,
        icon_id: row.try_get("icon_id")?,
    })
}

pub async fn all(ctx: &Ctx) -> Result<Vec<Team>, AppError> {
    let rows = sqlx::query(&format!("{SELECT} ORDER BY t.`order`, tl.title"))
        .bind(ctx.lang())
        .bind(ctx.lang())
        .fetch_all(&ctx.state.db)
        .await?;
    Ok(rows.iter().map(from_row).collect::<sqlx::Result<_>>()?)
}

pub async fn find(ctx: &Ctx, team_id: u32) -> Result<Option<Team>, AppError> {
    let row = sqlx::query(&format!("{SELECT} WHERE t.team_id = ?"))
        .bind(ctx.lang())
        .bind(ctx.lang())
        .bind(team_id)
        .fetch_optional(&ctx.state.db)
        .await?;
    Ok(row.as_ref().map(from_row).transpose()?)
}

/// Liste déroulante des équipes (pour les matchs, le palmarès, le recrutement).
pub async fn select(ctx: &Ctx, label: &str, name: &str, selected: &str, empty: Option<&str>) -> Result<String, AppError> {
    let teams = all(ctx).await?;
    let mut options: Vec<(String, String)> = empty.map(|e| (String::new(), e.to_owned())).into_iter().collect();
    options.extend(teams.iter().map(|t| (t.team_id.to_string(), text::decode(&t.title))));
    Ok(form::select(label, name, selected, options.iter().map(|(v, t)| (v.as_str(), t.as_str()))))
}

struct Player {
    user_id: u32,
    username: String,
    role: Option<String>,
    avatar: Option<String>,
    sex: Option<String>,
}

async fn players(ctx: &Ctx, team_id: u32) -> Result<Vec<Player>, AppError> {
    let rows = sqlx::query(
        "SELECT u.id, u.username, r.title AS role, f.path AS avatar, up.sex FROM nf_teams_users tu \
         INNER JOIN nf_user u ON u.id = tu.user_id AND u.deleted = '0' \
         LEFT JOIN nf_teams_roles r ON r.role_id = tu.role_id \
         LEFT JOIN nf_user_profile up ON up.id = u.id LEFT JOIN nf_file f ON f.id = up.avatar \
         WHERE tu.team_id = ? ORDER BY IFNULL(r.`order`, 999), r.title, u.username",
    )
    .bind(team_id)
    .fetch_all(&ctx.state.db)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(Player {
                user_id: row.try_get("id")?,
                username: row.try_get("username")?,
                role: row.try_get::<Option<String>, _>("role")?.map(|r| text::clean(&r)),
                avatar: row.try_get("avatar")?,
                sex: row.try_get("sex")?,
            })
        })
        .collect::<sqlx::Result<_>>()
        .map_err(Into::into)
}

fn team_card(ctx: &Ctx, team: &Team) -> String {
    let image = match &team.image {
        Some(path) => format!(
            "<a href=\"{}\"><img class=\"card-img-top\" src=\"{}\" alt=\"\" /></a>",
            ctx.url(&team.path()),
            text::encode(&ctx.url(path))
        ),
        None => String::new(),
    };
    format!(
        "{image}<div class=\"card-body\"><h5 class=\"card-title mb-1\"><a href=\"{}\">{}</a></h5>\
         <span class=\"badge badge-dark\">{}</span> <span class=\"text-muted\">{} {} joueur{}</span></div>",
        ctx.url(&team.path()),
        team.title,
        team.game_title,
        text::icon("fas fa-users"),
        team.players,
        if team.players > 1 { "s" } else { "" },
    )
}

pub async fn index(Extension(ctx): Ctxt) -> Reply {
    if !ctx.enabled("teams") {
        return layout::not_found(&ctx).await;
    }
    let teams = all(&ctx).await?;
    let mut blocks: Vec<Block> = teams
        .iter()
        .map(|team| Block::Panel(Panel::new().body_raw(team_card(&ctx, team))))
        .collect();
    if blocks.is_empty() {
        blocks.push(Block::Panel(
            Panel::new()
                .heading("Équipe", ICON)
                .style("card-info")
                .body("<div class=\"text-center\">Aucune équipe n'a été créée pour le moment</div>"),
        ));
    }
    layout::render(&ctx, Page::new("teams", TITLE, ICON).blocks(&blocks)).await
}

pub async fn show(Extension(ctx): Ctxt, Path((id, name)): Path<(u32, String)>) -> Reply {
    if !ctx.enabled("teams") {
        return layout::not_found(&ctx).await;
    }
    let team = match find(&ctx, id).await? {
        Some(team) if team.name == name => team,
        _ => return layout::not_found(&ctx).await,
    };
    let mut blocks = vec![Block::Panel(Panel::new().body_raw(format!(
        "{}{}",
        team_card(&ctx, &team),
        if team.description.is_empty() {
            String::new()
        } else {
            format!("<div class=\"card-body border-top\">{}</div>", bbcode::render(&team.description))
        }
    )))];

    let players = players(&ctx, id).await?;
    let list = if players.is_empty() {
        "<div class=\"card-body text-center\">Il n'y a pas encore de joueur dans cette équipe</div>".to_owned()
    } else {
        let rows: String = players
            .iter()
            .map(|p| {
                format!(
                    "<li class=\"list-group-item\"><div class=\"media align-items-center\">{}<div class=\"media-body ml-3\">\
                     <h6 class=\"mb-0\">{}</h6><small class=\"text-muted\">{}</small></div></div></li>",
                    user::avatar_html(ctx.base(), p.avatar.as_deref(), p.sex.as_deref(), false, ""),
                    user::link(ctx.base(), p.user_id, &p.username),
                    p.role.as_deref().unwrap_or(""),
                )
            })
            .collect();
        format!("<ul class=\"list-group list-group-flush\">{rows}</ul>")
    };
    blocks.push(Block::Panel(Panel::new().heading("Joueurs", "fas fa-users").body_raw(list)));
    blocks.push(Block::Html(super::events::team_matches(&ctx, id).await?));

    let page = Page::new("teams", TITLE, ICON)
        .title(team.title.clone())
        .crumb(team.title.clone(), team.path())
        .blocks(&blocks);
    layout::render(&ctx, page).await
}

/// Widget « Équipes ».
pub async fn widget(ctx: &Ctx) -> Result<Vec<Block>, AppError> {
    let teams = all(ctx).await?;
    let panel = Panel::new().heading(TITLE, ICON);
    if teams.is_empty() {
        return Ok(vec![Block::Panel(panel.body("Aucune équipe pour le moment"))]);
    }
    let items: String = teams
        .iter()
        .map(|t| {
            format!(
                "<li class=\"list-group-item\"><span class=\"badge badge-secondary float-right\">{}</span><a href=\"{}\">{}</a></li>",
                t.players,
                ctx.url(&t.path()),
                t.title
            )
        })
        .collect();
    Ok(vec![Block::Panel(
        panel.body_raw(format!("<ul class=\"list-group list-group-flush\">{items}</ul>")),
    )])
}

// ---------------------------------------------------------------------------
// Administration

pub async fn admin_list(Extension(ctx): Ctxt, Query(notice): Query<Notice>) -> Reply {
    admin_guard!(ctx, "teams");
    let mut body = String::new();
    for team in all(&ctx).await? {
        body.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td>{}</tr>",
            team.title,
            team.game_title,
            team.players,
            admin::row_actions(
                &ctx,
                &format!("admin/{}", team.path()),
                &format!("admin/teams/delete/{}", team.team_id),
                "Supprimer cette équipe ? Ses matchs et son palmarès seront supprimés."
            ),
        ));
    }
    let roles = sqlx::query("SELECT role_id, title FROM nf_teams_roles ORDER BY `order`, title")
        .fetch_all(&ctx.state.db)
        .await?;
    let mut role_rows = String::new();
    for row in &roles {
        role_rows.push_str(&format!(
            "<tr><td>{}</td><td class=\"text-right\">{}</td></tr>",
            text::clean(&row.try_get::<String, _>("title")?),
            admin::delete_button(&ctx, &format!("admin/teams/roles/delete/{}", row.try_get::<u32, _>("role_id")?), "Supprimer ce rôle ?"),
        ));
    }
    let content = format!(
        "<div class=\"row\"><div class=\"col-12 col-xl-8\">{}</div><div class=\"col-12 col-xl-4 mt-4 mt-xl-0\">\
         <div class=\"card\"><div class=\"card-body\"><h5>Rôles</h5><form method=\"post\" action=\"{}\" class=\"form-inline\">{}\
         <input type=\"text\" class=\"form-control mr-2\" name=\"title\" maxlength=\"100\" placeholder=\"Titre\" required />\
         <button type=\"submit\" class=\"btn btn-primary\">Ajouter</button></form></div>\
         <table class=\"table table-hover mb-0\"><tbody>{role_rows}</tbody></table></div></div></div>",
        admin::list_card(&ctx, TITLE, Some("admin/teams/add"), &["Titre", "Jeu", "Joueurs", ""], &body),
        ctx.url("admin/teams/roles/add"),
        form::csrf_field(&ctx),
    );
    admin::render(&ctx, "teams", TITLE, notice.message(), content)
}

struct TeamValues {
    title: String,
    game_id: String,
    description: String,
}

async fn team_form(ctx: &Ctx, action: &str, values: &TeamValues, team: Option<&Team>, error: Option<&str>) -> Reply {
    if games::all(ctx).await?.is_empty() {
        let content = format!(
            "<div class=\"alert alert-info\">Créez d'abord un <a href=\"{}\">jeu</a>.</div>",
            ctx.url("admin/games/add")
        );
        return admin::render(ctx, "teams", "Ajouter une équipe", None, content);
    }
    let fields = format!(
        "{}{}{}{}{}",
        input("Titre", "title", &values.title, 100, true, ""),
        games::select(ctx, "Jeu", "game_id", &values.game_id, None).await?,
        admin::image_field(ctx, "Image", "image", team.and_then(|t| t.image.as_deref()), ""),
        admin::image_field(ctx, "Icône", "icon", team.and_then(|t| t.icon.as_deref()), "Petite image carrée."),
        textarea("Description", "description", &values.description, 8, false),
    );
    let mut content = admin::upload_card(ctx, action, error, fields, "admin/teams");

    if let Some(team) = team {
        let mut rows = String::new();
        for p in players(ctx, team.team_id).await? {
            rows.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td class=\"text-right\">{}</td></tr>",
                text::clean(&p.username),
                p.role.as_deref().unwrap_or("—"),
                admin::delete_button(ctx, &format!("admin/teams/{}/players/delete/{}", team.team_id, p.user_id), "Retirer ce joueur de l'équipe ?"),
            ));
        }
        let roles = sqlx::query("SELECT role_id, title FROM nf_teams_roles ORDER BY `order`, title")
            .fetch_all(&ctx.state.db)
            .await?;
        let mut options = String::from("<option value=\"\">— Rôle —</option>");
        for row in &roles {
            options.push_str(&format!(
                "<option value=\"{}\">{}</option>",
                row.try_get::<u32, _>("role_id")?,
                text::clean(&row.try_get::<String, _>("title")?)
            ));
        }
        content.push_str(&format!(
            "<div class=\"card mt-4\"><div class=\"card-body\"><h5>Joueurs</h5>\
             <form method=\"post\" action=\"{}\" class=\"form-inline\">{}\
             <input type=\"text\" class=\"form-control mr-2\" name=\"username\" placeholder=\"Pseudo du membre\" required />\
             <select class=\"form-control mr-2\" name=\"role_id\">{options}</select>\
             <button type=\"submit\" class=\"btn btn-primary\">Ajouter</button></form></div>\
             <table class=\"table table-hover mb-0\"><thead><tr><th>Membre</th><th>Rôle</th><th></th></tr></thead><tbody>{rows}</tbody></table></div>",
            ctx.url(&format!("admin/teams/{}/players/add", team.team_id)),
            form::csrf_field(ctx),
        ));
    }
    admin::render(ctx, "teams", if team.is_some() { "Modifier l'équipe" } else { "Ajouter une équipe" }, None, content)
}

async fn validate(ctx: &Ctx, values: &TeamValues) -> Result<Result<(String, u32), &'static str>, AppError> {
    if values.title.is_empty() || text::url_title(&values.title).is_empty() {
        return Ok(Err("Le titre est obligatoire."));
    }
    if !fits(&values.title, 100) {
        return Ok(Err("Le titre est trop long (100 caractères maximum)."));
    }
    match values.game_id.parse::<u32>() {
        Ok(game_id) if games::exists(ctx, game_id).await? => Ok(Ok((text::url_title(&values.title), game_id))),
        _ => Ok(Err("Jeu invalide.")),
    }
}

fn values_from(data: &crate::core::files::FormData) -> TeamValues {
    TeamValues {
        title: data.text("title").trim().to_owned(),
        game_id: data.text("game_id").to_owned(),
        description: data.text("description").trim().to_owned(),
    }
}

pub async fn admin_add_get(Extension(ctx): Ctxt) -> Reply {
    admin_guard!(ctx, "teams");
    let values = TeamValues { title: String::new(), game_id: String::new(), description: String::new() };
    team_form(&ctx, "admin/teams/add", &values, None, None).await
}

/// Enregistre le formulaire (création si `team` est `None`).
async fn save(ctx: &Ctx, multipart: Multipart, team: Option<Team>) -> Reply {
    let mut data = admin::read_upload_form(ctx, multipart).await?;
    let values = values_from(&data);
    let action = match &team {
        Some(team) => format!("admin/{}", team.path()),
        None => "admin/teams/add".to_owned(),
    };
    let (name, game_id) = match validate(ctx, &values).await? {
        Ok(valid) => valid,
        Err(message) => return team_form(ctx, &action, &values, team.as_ref(), Some(message)).await,
    };
    let old = team.as_ref().map(|t| (t.image_id, t.icon_id)).unwrap_or((None, None));
    let image = match admin::image_change(ctx, &mut data, "image", "teams", Fit::MaxWidth(1600), old.0).await? {
        Ok(change) => change,
        Err(message) => return team_form(ctx, &action, &values, team.as_ref(), Some(message)).await,
    };
    let icon = match admin::image_change(ctx, &mut data, "icon", "teams/icons", Fit::Square(64), old.1).await? {
        Ok(change) => change,
        Err(message) => return team_form(ctx, &action, &values, team.as_ref(), Some(message)).await,
    };

    let mut tx = ctx.state.db.begin().await?;
    let team_id = match &team {
        Some(team) => {
            sqlx::query("UPDATE nf_teams SET name = ?, game_id = ?, image_id = ?, icon_id = ? WHERE team_id = ?")
                .bind(&name)
                .bind(game_id)
                .bind(image.value)
                .bind(icon.value)
                .bind(team.team_id)
                .execute(&mut *tx)
                .await?;
            team.team_id as u64
        }
        None => sqlx::query("INSERT INTO nf_teams (name, game_id, image_id, icon_id) VALUES (?, ?, ?, ?)")
            .bind(&name)
            .bind(game_id)
            .bind(image.value)
            .bind(icon.value)
            .execute(&mut *tx)
            .await?
            .last_insert_id(),
    };
    sqlx::query(
        "INSERT INTO nf_teams_lang (team_id, lang, title, description) VALUES (?, ?, ?, ?) \
         ON DUPLICATE KEY UPDATE title = VALUES(title), description = VALUES(description)",
    )
    .bind(team_id)
    .bind(ctx.lang())
    .bind(text::encode(&values.title))
    .bind(text::encode(&values.description))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    image.finish(ctx).await?;
    icon.finish(ctx).await?;
    if team.is_some() {
        admin::redirect(ctx, "admin/teams?ok=1")
    } else {
        // Sur la fiche : c'est là qu'on ajoute les joueurs.
        admin::redirect(ctx, &format!("admin/teams/{team_id}/{name}"))
    }
}

pub async fn admin_add_post(Extension(ctx): Ctxt, multipart: Multipart) -> Reply {
    admin_guard!(ctx, "teams");
    save(&ctx, multipart, None).await
}

pub async fn admin_edit_get(Extension(ctx): Ctxt, Path((id, _name)): Path<(u32, String)>) -> Reply {
    admin_guard!(ctx, "teams");
    let Some(team) = find(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let values = TeamValues {
        title: text::decode(&team.title),
        game_id: team.game_id.to_string(),
        description: text::decode(&team.description),
    };
    team_form(&ctx, &format!("admin/{}", team.path()), &values, Some(&team), None).await
}

pub async fn admin_edit_post(Extension(ctx): Ctxt, Path((id, _name)): Path<(u32, String)>, multipart: Multipart) -> Reply {
    admin_guard!(ctx, "teams");
    let Some(team) = find(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    save(&ctx, multipart, Some(team)).await
}

pub async fn admin_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx, "teams");
    admin::check_csrf(&ctx, &form.csrf)?;
    sqlx::query("DELETE FROM nf_teams WHERE team_id = ?")
        .bind(id)
        .execute(&ctx.state.db)
        .await?;
    admin::redirect(&ctx, "admin/teams?ok=1")
}

#[derive(Deserialize)]
pub struct PlayerForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    role_id: String,
}

pub async fn admin_player_add(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<PlayerForm>) -> Reply {
    admin_guard!(ctx, "teams");
    admin::check_csrf(&ctx, &form.csrf)?;
    let Some(team) = find(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let db = &ctx.state.db;
    let user_id: Option<u32> = sqlx::query_scalar("SELECT id FROM nf_user WHERE username = ? AND deleted = '0'")
        .bind(text::encode(form.username.trim()))
        .fetch_optional(db)
        .await?;
    let Some(user_id) = user_id else {
        return Err(AppError::BadRequest("Aucun membre ne porte ce pseudo."));
    };
    let role_id: Option<u32> = match form.role_id.parse::<u32>() {
        Ok(role_id) => sqlx::query_scalar("SELECT role_id FROM nf_teams_roles WHERE role_id = ?")
            .bind(role_id)
            .fetch_optional(db)
            .await?,
        Err(_) => None,
    };
    sqlx::query(
        "INSERT INTO nf_teams_users (team_id, user_id, role_id) VALUES (?, ?, ?) ON DUPLICATE KEY UPDATE role_id = VALUES(role_id)",
    )
    .bind(id)
    .bind(user_id)
    .bind(role_id)
    .execute(db)
    .await?;
    admin::redirect(&ctx, &format!("admin/{}", team.path()))
}

pub async fn admin_player_delete(Extension(ctx): Ctxt, Path((id, user_id)): Path<(u32, u32)>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx, "teams");
    admin::check_csrf(&ctx, &form.csrf)?;
    sqlx::query("DELETE FROM nf_teams_users WHERE team_id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(&ctx.state.db)
        .await?;
    admin::redirect(&ctx, &format!("admin/teams/{id}/x"))
}

#[derive(Deserialize)]
pub struct RoleForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    title: String,
}

pub async fn admin_role_add(Extension(ctx): Ctxt, Form(form): Form<RoleForm>) -> Reply {
    admin_guard!(ctx, "teams");
    admin::check_csrf(&ctx, &form.csrf)?;
    let title = form.title.trim();
    if !title.is_empty() && fits(title, 100) {
        sqlx::query("INSERT INTO nf_teams_roles (title, `order`) VALUES (?, 0)")
            .bind(text::encode(title))
            .execute(&ctx.state.db)
            .await?;
    }
    admin::redirect(&ctx, "admin/teams")
}

pub async fn admin_role_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx, "teams");
    admin::check_csrf(&ctx, &form.csrf)?;
    sqlx::query("DELETE FROM nf_teams_roles WHERE role_id = ?")
        .bind(id)
        .execute(&ctx.state.db)
        .await?;
    admin::redirect(&ctx, "admin/teams")
}
