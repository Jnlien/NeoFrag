//! Module Jeux : jeux, cartes et modes de jeu, utilisés par les équipes, les matchs et
//! le palmarès. Porté de `modules/games` (administration uniquement, comme l'original).

use super::admin::{self, admin_guard, Ctxt, CsrfOnly, Notice, Reply};
use crate::{
    core::{
        ctx::Ctx,
        files::Fit,
        form::{self, fits, input},
        layout, text,
    },
    error::AppError,
};
use axum::{
    extract::{Multipart, Path, Query},
    Extension, Form,
};
use sqlx::Row;

pub struct Game {
    pub game_id: u32,
    pub name: String,
    pub title: String,
    pub image: Option<String>,
    pub icon: Option<String>,
}

const SELECT: &str = "SELECT g.game_id, g.name, gl.title, g.image_id, g.icon_id, fi.path AS image, fc.path AS icon \
     FROM nf_games g INNER JOIN nf_games_lang gl ON gl.game_id = g.game_id AND gl.lang = ? \
     LEFT JOIN nf_file fi ON fi.id = g.image_id LEFT JOIN nf_file fc ON fc.id = g.icon_id";

fn from_row(row: &sqlx::mysql::MySqlRow) -> sqlx::Result<Game> {
    Ok(Game {
        game_id: row.try_get("game_id")?,
        name: row.try_get("name")?,
        title: text::clean(&row.try_get::<String, _>("title")?),
        image: row.try_get("image")?,
        icon: row.try_get("icon")?,
    })
}

pub async fn all(ctx: &Ctx) -> Result<Vec<Game>, AppError> {
    let rows = sqlx::query(&format!("{SELECT} ORDER BY gl.title"))
        .bind(ctx.lang())
        .fetch_all(&ctx.state.db)
        .await?;
    Ok(rows.iter().map(from_row).collect::<sqlx::Result<_>>()?)
}

/// Liste déroulante des jeux. `empty` : libellé d'un choix vide, s'il est permis.
pub async fn select(ctx: &Ctx, label: &str, name: &str, selected: &str, empty: Option<&str>) -> Result<String, AppError> {
    let games = all(ctx).await?;
    let mut options: Vec<(String, String)> = empty.map(|e| (String::new(), e.to_owned())).into_iter().collect();
    options.extend(games.iter().map(|g| (g.game_id.to_string(), text::decode(&g.title))));
    Ok(form::select(label, name, selected, options.iter().map(|(v, t)| (v.as_str(), t.as_str()))))
}

pub async fn exists(ctx: &Ctx, game_id: u32) -> Result<bool, AppError> {
    Ok(sqlx::query("SELECT 1 FROM nf_games WHERE game_id = ?")
        .bind(game_id)
        .fetch_optional(&ctx.state.db)
        .await?
        .is_some())
}

// ---------------------------------------------------------------------------
// Administration

pub async fn admin_list(Extension(ctx): Ctxt, Query(notice): Query<Notice>) -> Reply {
    admin_guard!(ctx, "games");
    let rows = sqlx::query(
        "SELECT g.game_id, g.name, gl.title, \
                (SELECT COUNT(*) FROM nf_games_maps m WHERE m.game_id = g.game_id) AS maps, \
                (SELECT COUNT(*) FROM nf_games_modes m WHERE m.game_id = g.game_id) AS modes \
         FROM nf_games g INNER JOIN nf_games_lang gl ON gl.game_id = g.game_id AND gl.lang = ? ORDER BY gl.title",
    )
    .bind(ctx.lang())
    .fetch_all(&ctx.state.db)
    .await?;
    let mut body = String::new();
    for row in &rows {
        let id: u32 = row.try_get("game_id")?;
        let name: String = row.try_get("name")?;
        body.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td>{}</tr>",
            text::clean(&row.try_get::<String, _>("title")?),
            row.try_get::<i64, _>("maps")?,
            row.try_get::<i64, _>("modes")?,
            admin::row_actions(
                &ctx,
                &format!("admin/games/{id}/{name}"),
                &format!("admin/games/delete/{id}"),
                "Supprimer ce jeu ? Ses cartes, ses modes, mais aussi les équipes et le palmarès qui lui sont rattachés seront supprimés."
            ),
        ));
    }
    let content = admin::list_card(&ctx, "Jeux", Some("admin/games/add"), &["Titre", "Cartes", "Modes", ""], &body);
    admin::render(&ctx, "games", "Jeux", notice.message(), content)
}

async fn game_form(ctx: &Ctx, action: &str, title_value: &str, game: Option<&Game>, error: Option<&str>) -> Reply {
    let fields = format!(
        "{}{}{}",
        input("Titre", "title", title_value, 100, true, ""),
        admin::image_field(ctx, "Image", "image", game.and_then(|g| g.image.as_deref()), ""),
        admin::image_field(ctx, "Icône", "icon", game.and_then(|g| g.icon.as_deref()), "Petite image carrée."),
    );
    let mut content = admin::upload_card(ctx, action, error, fields, "admin/games");

    // Cartes et modes : gérés depuis la fiche du jeu.
    if let Some(game) = game {
        let db = &ctx.state.db;
        let maps = sqlx::query("SELECT map_id, title FROM nf_games_maps WHERE game_id = ? ORDER BY title")
            .bind(game.game_id)
            .fetch_all(db)
            .await?;
        let modes = sqlx::query("SELECT mode_id, title FROM nf_games_modes WHERE game_id = ? ORDER BY title")
            .bind(game.game_id)
            .fetch_all(db)
            .await?;
        let sub = |kind: &str, label: &str, id_col: &str, rows: &[sqlx::mysql::MySqlRow]| -> sqlx::Result<String> {
            let mut body = String::new();
            for row in rows {
                body.push_str(&format!(
                    "<tr><td>{}</td><td class=\"text-right\">{}</td></tr>",
                    text::clean(&row.try_get::<String, _>("title")?),
                    admin::delete_button(ctx, &format!("admin/games/{kind}/delete/{}", row.try_get::<u32, _>(id_col)?), "Supprimer ?"),
                ));
            }
            Ok(format!(
                "<div class=\"col-12 col-lg-6\"><div class=\"card mt-4\"><div class=\"card-body\"><h5>{label}</h5>\
                 <form method=\"post\" action=\"{}\" class=\"form-inline\">{}\
                 <input type=\"text\" class=\"form-control mr-2\" name=\"title\" maxlength=\"100\" placeholder=\"Titre\" required />\
                 <button type=\"submit\" class=\"btn btn-primary\">Ajouter</button></form></div>\
                 <table class=\"table table-hover mb-0\"><tbody>{body}</tbody></table></div></div>",
                ctx.url(&format!("admin/games/{}/{kind}/add", game.game_id)),
                form::csrf_field(ctx),
            ))
        };
        content.push_str(&format!(
            "<div class=\"row\">{}{}</div>",
            sub("maps", "Cartes", "map_id", &maps)?,
            sub("modes", "Modes", "mode_id", &modes)?
        ));
    }
    admin::render(ctx, "games", if game.is_some() { "Modifier le jeu" } else { "Ajouter un jeu" }, None, content)
}

fn validate_title(title: &str) -> Result<String, &'static str> {
    if title.is_empty() || text::url_title(title).is_empty() {
        return Err("Le titre est obligatoire.");
    }
    if !fits(title, 100) {
        return Err("Le titre est trop long (100 caractères maximum).");
    }
    Ok(text::url_title(title))
}

async fn find(ctx: &Ctx, game_id: u32) -> Result<Option<(Game, Option<u32>, Option<u32>)>, AppError> {
    let row = sqlx::query(&format!("{SELECT} WHERE g.game_id = ?"))
        .bind(ctx.lang())
        .bind(game_id)
        .fetch_optional(&ctx.state.db)
        .await?;
    Ok(match row {
        Some(row) => Some((from_row(&row)?, row.try_get("image_id")?, row.try_get("icon_id")?)),
        None => None,
    })
}

pub async fn admin_add_get(Extension(ctx): Ctxt) -> Reply {
    admin_guard!(ctx, "games");
    game_form(&ctx, "admin/games/add", "", None, None).await
}

pub async fn admin_add_post(Extension(ctx): Ctxt, multipart: Multipart) -> Reply {
    admin_guard!(ctx, "games");
    let mut data = admin::read_upload_form(&ctx, multipart).await?;
    let title = data.text("title").trim().to_owned();
    let name = match validate_title(&title) {
        Ok(name) => name,
        Err(message) => return game_form(&ctx, "admin/games/add", &title, None, Some(message)).await,
    };
    let image = match admin::image_change(&ctx, &mut data, "image", "games", Fit::MaxWidth(1600), None).await? {
        Ok(change) => change,
        Err(message) => return game_form(&ctx, "admin/games/add", &title, None, Some(message)).await,
    };
    let icon = match admin::image_change(&ctx, &mut data, "icon", "games/icons", Fit::Square(64), None).await? {
        Ok(change) => change,
        Err(message) => return game_form(&ctx, "admin/games/add", &title, None, Some(message)).await,
    };
    let mut tx = ctx.state.db.begin().await?;
    let created = sqlx::query("INSERT INTO nf_games (name, image_id, icon_id) VALUES (?, ?, ?)")
        .bind(&name)
        .bind(image.value)
        .bind(icon.value)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO nf_games_lang (game_id, lang, title) VALUES (?, ?, ?)")
        .bind(created.last_insert_id())
        .bind(ctx.lang())
        .bind(text::encode(&title))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    admin::redirect(&ctx, &format!("admin/games/{}/{name}", created.last_insert_id()))
}

pub async fn admin_edit_get(Extension(ctx): Ctxt, Path((id, _name)): Path<(u32, String)>) -> Reply {
    admin_guard!(ctx, "games");
    let Some((game, _, _)) = find(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let title = text::decode(&game.title);
    game_form(&ctx, &format!("admin/games/{id}/{}", game.name), &title, Some(&game), None).await
}

pub async fn admin_edit_post(Extension(ctx): Ctxt, Path((id, _name)): Path<(u32, String)>, multipart: Multipart) -> Reply {
    admin_guard!(ctx, "games");
    let mut data = admin::read_upload_form(&ctx, multipart).await?;
    let Some((game, old_image, old_icon)) = find(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let action = format!("admin/games/{id}/{}", game.name);
    let title = data.text("title").trim().to_owned();
    let name = match validate_title(&title) {
        Ok(name) => name,
        Err(message) => return game_form(&ctx, &action, &title, Some(&game), Some(message)).await,
    };
    let image = match admin::image_change(&ctx, &mut data, "image", "games", Fit::MaxWidth(1600), old_image).await? {
        Ok(change) => change,
        Err(message) => return game_form(&ctx, &action, &title, Some(&game), Some(message)).await,
    };
    let icon = match admin::image_change(&ctx, &mut data, "icon", "games/icons", Fit::Square(64), old_icon).await? {
        Ok(change) => change,
        Err(message) => return game_form(&ctx, &action, &title, Some(&game), Some(message)).await,
    };
    let mut tx = ctx.state.db.begin().await?;
    sqlx::query("UPDATE nf_games SET name = ?, image_id = ?, icon_id = ? WHERE game_id = ?")
        .bind(&name)
        .bind(image.value)
        .bind(icon.value)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO nf_games_lang (game_id, lang, title) VALUES (?, ?, ?) ON DUPLICATE KEY UPDATE title = VALUES(title)",
    )
    .bind(id)
    .bind(ctx.lang())
    .bind(text::encode(&title))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    image.finish(&ctx).await?;
    icon.finish(&ctx).await?;
    admin::redirect(&ctx, "admin/games?ok=1")
}

pub async fn admin_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx, "games");
    admin::check_csrf(&ctx, &form.csrf)?;
    sqlx::query("DELETE FROM nf_games WHERE game_id = ?")
        .bind(id)
        .execute(&ctx.state.db)
        .await?;
    admin::redirect(&ctx, "admin/games?ok=1")
}

#[derive(serde::Deserialize)]
pub struct TitleForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    title: String,
}

/// Ajoute une carte (`maps`) ou un mode (`modes`) à un jeu.
pub async fn admin_sub_add(Extension(ctx): Ctxt, Path((id, kind)): Path<(u32, String)>, Form(form): Form<TitleForm>) -> Reply {
    admin_guard!(ctx, "games");
    admin::check_csrf(&ctx, &form.csrf)?;
    let Some((game, _, _)) = find(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let title = form.title.trim();
    let sql = match kind.as_str() {
        "maps" => "INSERT INTO nf_games_maps (game_id, title) VALUES (?, ?)",
        "modes" => "INSERT INTO nf_games_modes (game_id, title) VALUES (?, ?)",
        _ => return layout::not_found(&ctx).await,
    };
    if !title.is_empty() && fits(title, 100) {
        sqlx::query(sql).bind(id).bind(text::encode(title)).execute(&ctx.state.db).await?;
    }
    admin::redirect(&ctx, &format!("admin/games/{id}/{}", game.name))
}

pub async fn admin_sub_delete(Extension(ctx): Ctxt, Path((kind, id)): Path<(String, u32)>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx, "games");
    admin::check_csrf(&ctx, &form.csrf)?;
    let (select, delete) = match kind.as_str() {
        "maps" => ("SELECT game_id FROM nf_games_maps WHERE map_id = ?", "DELETE FROM nf_games_maps WHERE map_id = ?"),
        "modes" => ("SELECT game_id FROM nf_games_modes WHERE mode_id = ?", "DELETE FROM nf_games_modes WHERE mode_id = ?"),
        _ => return layout::not_found(&ctx).await,
    };
    let game_id: Option<u32> = sqlx::query_scalar(select).bind(id).fetch_optional(&ctx.state.db).await?;
    sqlx::query(delete).bind(id).execute(&ctx.state.db).await?;
    match game_id {
        Some(game_id) => admin::redirect(&ctx, &format!("admin/games/{game_id}/x")),
        None => admin::redirect(&ctx, "admin/games"),
    }
}
