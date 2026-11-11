//! Widgets Membres, portés de `widgets/members`.

use crate::{
    core::{
        ctx::Ctx,
        panel::{Block, Panel},
        php::Value,
        session, text, user,
    },
    error::AppError,
};
use sqlx::Row;

/// Derniers membres inscrits.
pub async fn index(ctx: &Ctx) -> Result<Vec<Block>, AppError> {
    let rows = sqlx::query(
        "SELECT id, username FROM nf_user WHERE deleted = '0' ORDER BY registration_date DESC, id DESC LIMIT 5",
    )
    .fetch_all(&ctx.state.db)
    .await?;

    let panel = Panel::new().heading("Derniers membres", "");
    if rows.is_empty() {
        return Ok(vec![Block::Panel(panel.body("Aucun membre pour le moment"))]);
    }
    let mut items = String::new();
    for row in rows {
        let id: u32 = row.try_get("id")?;
        let username: String = row.try_get("username")?;
        items.push_str(&format!(
            "<li class=\"list-group-item\">{} {}</li>",
            text::icon("fas fa-user"),
            user::link(ctx.base(), id, &username)
        ));
    }
    let footer = format!(
        "<a href=\"{}\">{} Liste des membres</a>",
        ctx.url("members"),
        text::icon("far fa-arrow-alt-circle-right")
    );
    Ok(vec![Block::Panel(
        panel
            .body_raw(format!("<ul class=\"list-group list-group-flush\">{items}</ul>"))
            .footer(footer, true),
    )])
}

/// Qui est en ligne ?
pub async fn online(ctx: &Ctx) -> Result<Vec<Block>, AppError> {
    let rows = sqlx::query(
        "SELECT u.id, u.username, (u.admin = '1') AS admin FROM nf_session s \
         INNER JOIN nf_user u ON u.id = s.user_id AND u.deleted = '0' \
         WHERE s.last_activity > DATE_SUB(NOW(), INTERVAL 5 MINUTE) \
         GROUP BY u.id, u.username, u.admin ORDER BY u.username",
    )
    .fetch_all(&ctx.state.db)
    .await?;

    let (mut admins, mut members) = (Vec::new(), Vec::new());
    for row in rows {
        let link = user::link(ctx.base(), row.try_get("id")?, &row.try_get::<String, _>("username")?);
        if row.try_get::<i64, _>("admin")? != 0 {
            admins.push(link);
        } else {
            members.push(link);
        }
    }
    let sessions = session::online_count(ctx).await?;
    let visitors = (sessions - admins.len() as i64 - members.len() as i64).max(0);

    let line = |label: &str, count: usize, names: &[String]| {
        format!(
            "<li class=\"list-group-item\"><span class=\"badge badge-secondary float-right\">{count}</span>{label}{}</li>",
            if names.is_empty() { String::new() } else { format!("<br /><small>{}</small>", names.join(", ")) }
        )
    };
    let body = format!(
        "<ul class=\"list-group list-group-flush\">{}{}{}</ul>",
        line("Administrateurs", admins.len(), &admins),
        line("Membres", members.len(), &members),
        line("Visiteurs", visitors as usize, &[]),
    );
    Ok(vec![Block::Panel(
        Panel::new().heading("Qui est en ligne ?", "").body_raw(body),
    )])
}

pub async fn online_mini(ctx: &Ctx, settings: &Value) -> Result<String, AppError> {
    let count = session::online_count(ctx).await?;
    let align = match settings.get_str("align") {
        Some(align) if !align.is_empty() => text::css_classes(align),
        _ => "float-right".to_owned(),
    };
    Ok(format!(
        "<div class=\"text-grey {align}\">{} {count} personne{} en ligne</div>",
        text::icon("fas fa-globe"),
        if count > 1 { "s" } else { "" }
    ))
}
