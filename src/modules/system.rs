//! Administration du système : modules et widgets (`addons`), groupes, permissions
//! (`access`), statistiques et supervision (`monitoring`).
//!
//! Simplifications par rapport à l'original :
//! - Modules : activation / désactivation uniquement (pas d'installation par archive) ;
//! - Permissions : réglage par groupe (les règles nominatives existantes sont conservées) ;
//! - Statistiques : tableau mensuel sur douze mois au lieu des graphiques Highcharts ;
//! - Supervision : état du serveur et de la base (pas de mise à jour automatique).

use super::admin::{self, admin_guard, Ctxt, CsrfOnly, Notice, Reply};
use crate::{
    core::{
        access::DELEGABLE,
        ctx::Ctx,
        form::{self, checkbox, fits, input},
        layout, mail,
        php::{self, Value},
        text,
    },
    error::AppError,
};
use axum::{
    extract::{Path, Query},
    Extension, Form,
};
use serde::Deserialize;
use sqlx::Row;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Modules et widgets

/// Indispensables au fonctionnement du site : jamais désactivables.
const CORE_MODULES: [&str; 12] = [
    "access", "addons", "admin", "comments", "live_editor", "monitoring", "pages", "settings", "statistics", "tools", "user", "members",
];
const CORE_WIDGETS: [&str; 2] = ["module", "navigation"];

const MODULE_TITLES: [(&str, &str); 24] = [
    ("access", "Permissions"),
    ("addons", "Modules"),
    ("admin", "Administration"),
    ("awards", "Palmarès"),
    ("comments", "Commentaires"),
    ("contact", "Contact"),
    ("events", "Événements"),
    ("forum", "Forum"),
    ("gallery", "Galerie"),
    ("games", "Jeux"),
    ("live_editor", "Live Editor"),
    ("members", "Membres"),
    ("monitoring", "Supervision"),
    ("news", "Actualités"),
    ("pages", "Pages"),
    ("partners", "Partenaires"),
    ("recruits", "Recrutement"),
    ("search", "Recherche"),
    ("settings", "Paramètres"),
    ("statistics", "Statistiques"),
    ("talks", "Discussions"),
    ("teams", "Équipes"),
    ("tools", "Outils"),
    ("user", "Utilisateurs"),
];

fn is_core(kind: &str, name: &str) -> bool {
    match kind {
        "module" => CORE_MODULES.contains(&name),
        _ => CORE_WIDGETS.contains(&name),
    }
}

fn addon_enabled(data: &str) -> bool {
    php::unserialize(data)
        .ok()
        .and_then(|value| value.get("enabled").map(Value::truthy))
        .unwrap_or(true)
}

/// Réécrit `data` en changeant seulement `enabled` (les autres réglages sont conservés).
fn with_enabled(data: &str, enabled: bool) -> String {
    let mut pairs = match php::unserialize(data) {
        Ok(Value::Array(pairs)) => pairs,
        _ => Vec::new(),
    };
    let key = Value::Str("enabled".to_owned());
    match pairs.iter_mut().find(|(k, _)| *k == key) {
        Some(pair) => pair.1 = Value::Bool(enabled),
        None => pairs.push((key, Value::Bool(enabled))),
    }
    php::serialize(&Value::Array(pairs))
}

pub async fn addons(Extension(ctx): Ctxt, Query(notice): Query<Notice>) -> Reply {
    admin_guard!(ctx);
    let rows = sqlx::query(
        "SELECT a.id, a.name, t.name AS kind, IFNULL(a.data, '') AS data FROM nf_addon a \
         INNER JOIN nf_addon_type t ON t.id = a.type_id WHERE t.name IN ('module', 'widget') ORDER BY a.name",
    )
    .fetch_all(&ctx.state.db)
    .await?;
    let mut tables: HashMap<&str, String> = HashMap::new();
    for row in &rows {
        let id: u32 = row.try_get("id")?;
        let name: String = row.try_get("name")?;
        let kind: String = row.try_get("kind")?;
        let enabled = addon_enabled(&row.try_get::<String, _>("data")?);
        let title = MODULE_TITLES.iter().find(|(n, _)| *n == name).map(|(_, t)| *t).unwrap_or("");
        let action = if is_core(&kind, &name) {
            "<small class=\"text-muted\">Indispensable</small>".to_owned()
        } else {
            format!(
                "<form method=\"post\" action=\"{}\" class=\"d-inline\">{}<button type=\"submit\" class=\"btn btn-sm btn-outline-{}\">{}</button></form>",
                ctx.url(&format!("admin/addons/toggle/{id}")),
                form::csrf_field(&ctx),
                if enabled { "danger" } else { "success" },
                if enabled { "Désactiver" } else { "Activer" },
            )
        };
        let key = if kind == "module" { "module" } else { "widget" };
        tables.entry(key).or_default().push_str(&format!(
            "<tr><td><code>{}</code></td><td>{title}</td><td>{}</td><td class=\"text-right\">{action}</td></tr>",
            text::encode(&name),
            admin::badge(enabled, "Activé", "Désactivé"),
        ));
    }
    let empty = String::new();
    let content = format!(
        "<div class=\"row\"><div class=\"col-12 col-xl-6\">{}</div><div class=\"col-12 col-xl-6 mt-4 mt-xl-0\">{}</div></div>",
        admin::list_card(&ctx, "Modules", None, &["Nom", "Titre", "État", ""], tables.get("module").unwrap_or(&empty)),
        admin::list_card(&ctx, "Widgets", None, &["Nom", "", "État", ""], tables.get("widget").unwrap_or(&empty)),
    );
    admin::render(&ctx, "addons", "Modules", notice.message(), content)
}

pub async fn addon_toggle(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx);
    admin::check_csrf(&ctx, &form.csrf)?;
    let row = sqlx::query(
        "SELECT a.name, t.name AS kind, IFNULL(a.data, '') AS data FROM nf_addon a \
         INNER JOIN nf_addon_type t ON t.id = a.type_id WHERE a.id = ? AND t.name IN ('module', 'widget')",
    )
    .bind(id)
    .fetch_optional(&ctx.state.db)
    .await?;
    let Some(row) = row else {
        return layout::not_found(&ctx).await;
    };
    let name: String = row.try_get("name")?;
    let kind: String = row.try_get("kind")?;
    if is_core(&kind, &name) {
        return layout::forbidden(&ctx).await;
    }
    let data: String = row.try_get("data")?;
    sqlx::query("UPDATE nf_addon SET data = ? WHERE id = ?")
        .bind(with_enabled(&data, !addon_enabled(&data)))
        .bind(id)
        .execute(&ctx.state.db)
        .await?;
    admin::redirect(&ctx, "admin/addons?ok=1")
}

// ---------------------------------------------------------------------------
// Groupes

struct Group {
    group_id: u32,
    title: String,
    color: String,
    icon: String,
    hidden: bool,
}

async fn groups(ctx: &Ctx) -> Result<Vec<Group>, AppError> {
    let rows = sqlx::query(
        "SELECT g.group_id, IFNULL(gl.title, g.name) AS title, g.color, g.icon, (g.hidden = '1') AS is_hidden \
         FROM nf_groups g LEFT JOIN nf_groups_lang gl ON gl.group_id = g.group_id AND gl.lang = ? \
         WHERE g.auto = '0' ORDER BY g.`order`, g.group_id",
    )
    .bind(ctx.lang())
    .fetch_all(&ctx.state.db)
    .await?;
    let mut list = Vec::new();
    for row in &rows {
        list.push(Group {
            group_id: row.try_get("group_id")?,
            title: text::clean(&row.try_get::<String, _>("title")?),
            color: row.try_get("color")?,
            icon: row.try_get("icon")?,
            hidden: row.try_get::<i64, _>("is_hidden")? != 0,
        });
    }
    Ok(list)
}

const COLORS: [&str; 8] = ["primary", "secondary", "success", "info", "warning", "danger", "light", "dark"];

fn group_label(group: &Group) -> String {
    let color = if COLORS.contains(&group.color.as_str()) { group.color.as_str() } else { "secondary" };
    format!("<span class=\"badge badge-{color}\">{} {}</span>", text::icon(&group.icon), group.title)
}

pub async fn groups_list(Extension(ctx): Ctxt, Query(notice): Query<Notice>) -> Reply {
    admin_guard!(ctx);
    let mut body = String::new();
    for group in groups(&ctx).await? {
        let members: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nf_users_groups WHERE group_id = ?")
            .bind(group.group_id)
            .fetch_one(&ctx.state.db)
            .await?;
        body.push_str(&format!(
            "<tr><td>{}</td><td>{members}</td><td>{}</td>{}</tr>",
            group_label(&group),
            if group.hidden { "Masqué" } else { "" },
            admin::row_actions(
                &ctx,
                &format!("admin/user/groups/{}", group.group_id),
                &format!("admin/user/groups/delete/{}", group.group_id),
                "Supprimer ce groupe ?"
            ),
        ));
    }
    let content = format!(
        "<p class=\"text-muted\">Les groupes « Administrateurs », « Membres » et « Visiteurs » sont automatiques. \
         <a href=\"{}\">Retour aux membres</a></p>{}",
        ctx.url("admin/user"),
        admin::list_card(&ctx, "Groupes", Some("admin/user/groups/add"), &["Groupe", "Membres", "", ""], &body),
    );
    admin::render(&ctx, "user", "Groupes", notice.message(), content)
}

#[derive(Deserialize, Default)]
pub struct GroupForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    color: String,
    #[serde(default)]
    icon: String,
    hidden: Option<String>,
}

async fn group_members_card(ctx: &Ctx, group_id: u32) -> Result<String, AppError> {
    let rows = sqlx::query(
        "SELECT u.id, u.username FROM nf_users_groups ug INNER JOIN nf_user u ON u.id = ug.user_id \
         WHERE ug.group_id = ? AND u.deleted = '0' ORDER BY u.username",
    )
    .bind(group_id)
    .fetch_all(&ctx.state.db)
    .await?;
    let mut body = String::new();
    for row in &rows {
        let user_id: u32 = row.try_get("id")?;
        body.push_str(&format!(
            "<tr><td>{}</td><td class=\"text-right\">{}</td></tr>",
            text::clean(&row.try_get::<String, _>("username")?),
            admin::delete_button(ctx, &format!("admin/user/groups/{group_id}/members/delete/{user_id}"), "Retirer ce membre du groupe ?"),
        ));
    }
    Ok(format!(
        "<div class=\"card mt-4\"><div class=\"card-body\"><h5>Membres du groupe</h5>\
         <form method=\"post\" action=\"{}\" class=\"form-inline\">{}<input type=\"text\" class=\"form-control mr-2\" name=\"username\" placeholder=\"Nom d'utilisateur\" maxlength=\"100\" required />\
         <button type=\"submit\" class=\"btn btn-primary\">Ajouter</button></form></div>\
         <table class=\"table mb-0\"><tbody>{body}</tbody></table></div>",
        ctx.url(&format!("admin/user/groups/{group_id}/members/add")),
        form::csrf_field(ctx),
    ))
}

async fn group_form(ctx: &Ctx, group_id: Option<u32>, form: &GroupForm, error: Option<&str>, notice: Option<&str>) -> Reply {
    let fields = format!(
        "{}{}{}{}",
        input("Nom", "title", &form.title, 100, true, ""),
        form::select("Couleur", "color", &form.color, COLORS.iter().map(|c| (*c, *c))),
        input("Icône", "icon", &form.icon, 50, false, "Ex. fas fa-star"),
        checkbox("Groupe masqué", "hidden", form.hidden.is_some()),
    );
    let action = match group_id {
        Some(id) => format!("admin/user/groups/{id}"),
        None => "admin/user/groups/add".to_owned(),
    };
    let mut content = admin::form_card(ctx, &action, error, fields, "admin/user/groups");
    if let Some(id) = group_id {
        content.push_str(&group_members_card(ctx, id).await?);
    }
    admin::render(ctx, "user", if group_id.is_some() { "Modifier le groupe" } else { "Ajouter un groupe" }, notice, content)
}

pub async fn group_add_get(Extension(ctx): Ctxt) -> Reply {
    admin_guard!(ctx);
    let form = GroupForm { color: "primary".to_owned(), ..GroupForm::default() };
    group_form(&ctx, None, &form, None, None).await
}

pub async fn group_edit_get(Extension(ctx): Ctxt, Path(id): Path<u32>, Query(notice): Query<Notice>) -> Reply {
    admin_guard!(ctx);
    let Some(group) = groups(&ctx).await?.into_iter().find(|g| g.group_id == id) else {
        return layout::not_found(&ctx).await;
    };
    let form = GroupForm {
        csrf: String::new(),
        title: text::decode(&group.title),
        color: group.color,
        icon: group.icon,
        hidden: group.hidden.then(String::new),
    };
    group_form(&ctx, Some(id), &form, None, notice.message()).await
}

async fn group_save(ctx: &Ctx, group_id: Option<u32>, mut form: GroupForm) -> Reply {
    admin::check_csrf(ctx, &form.csrf)?;
    form.title = form.title.trim().to_owned();
    form.icon = form.icon.trim().to_owned();
    let error = if form.title.is_empty() || !fits(&form.title, 100) {
        Some("Le nom est obligatoire (100 caractères maximum).")
    } else if !COLORS.contains(&form.color.as_str()) {
        Some("Couleur invalide.")
    } else if !form.icon.is_empty() && (text::icon(&form.icon).is_empty() || form.icon.len() > 50) {
        Some("Icône invalide (ex. fas fa-star).")
    } else {
        None
    };
    if error.is_some() {
        return group_form(ctx, group_id, &form, error, None).await;
    }
    let hidden = if form.hidden.is_some() { "1" } else { "0" };
    let name = text::url_title(&form.title);
    let mut tx = ctx.state.db.begin().await?;
    let id = match group_id {
        Some(id) => {
            sqlx::query("UPDATE nf_groups SET name = ?, color = ?, icon = ?, hidden = ? WHERE group_id = ? AND auto = '0'")
                .bind(&name)
                .bind(&form.color)
                .bind(&form.icon)
                .bind(hidden)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            u64::from(id)
        }
        None => sqlx::query(
            "INSERT INTO nf_groups (name, color, icon, hidden, auto, `order`) \
             SELECT ?, ?, ?, ?, '0', IFNULL(MAX(`order`), 0) + 1 FROM nf_groups",
        )
        .bind(&name)
        .bind(&form.color)
        .bind(&form.icon)
        .bind(hidden)
        .execute(&mut *tx)
        .await?
        .last_insert_id(),
    };
    sqlx::query("INSERT INTO nf_groups_lang (group_id, lang, title) VALUES (?, ?, ?) ON DUPLICATE KEY UPDATE title = VALUES(title)")
        .bind(id)
        .bind(ctx.lang())
        .bind(text::encode(&form.title))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    admin::redirect(ctx, "admin/user/groups?ok=1")
}

pub async fn group_add_post(Extension(ctx): Ctxt, Form(form): Form<GroupForm>) -> Reply {
    admin_guard!(ctx);
    group_save(&ctx, None, form).await
}

pub async fn group_edit_post(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<GroupForm>) -> Reply {
    admin_guard!(ctx);
    if !groups(&ctx).await?.iter().any(|g| g.group_id == id) {
        return layout::not_found(&ctx).await;
    }
    group_save(&ctx, Some(id), form).await
}

pub async fn group_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx);
    admin::check_csrf(&ctx, &form.csrf)?;
    let mut tx = ctx.state.db.begin().await?;
    // Les règles de permission visant ce groupe n'ont plus de sens.
    sqlx::query("DELETE FROM nf_access_details WHERE type = 'group' AND entity = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM nf_groups WHERE group_id = ? AND auto = '0'")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    admin::redirect(&ctx, "admin/user/groups?ok=1")
}

#[derive(Deserialize)]
pub struct MemberForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    username: String,
}

pub async fn group_member_add(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<MemberForm>) -> Reply {
    admin_guard!(ctx);
    admin::check_csrf(&ctx, &form.csrf)?;
    if !groups(&ctx).await?.iter().any(|g| g.group_id == id) {
        return layout::not_found(&ctx).await;
    }
    let user_id: Option<u32> = sqlx::query_scalar("SELECT id FROM nf_user WHERE username = ? AND deleted = '0'")
        .bind(text::encode(form.username.trim()))
        .fetch_optional(&ctx.state.db)
        .await?;
    if let Some(user_id) = user_id {
        sqlx::query("INSERT IGNORE INTO nf_users_groups (user_id, group_id) VALUES (?, ?)")
            .bind(user_id)
            .bind(id)
            .execute(&ctx.state.db)
            .await?;
    }
    admin::redirect(&ctx, &format!("admin/user/groups/{id}{}", if user_id.is_some() { "?ok=1" } else { "" }))
}

pub async fn group_member_delete(Extension(ctx): Ctxt, Path((id, user_id)): Path<(u32, u32)>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx);
    admin::check_csrf(&ctx, &form.csrf)?;
    sqlx::query("DELETE FROM nf_users_groups WHERE group_id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(&ctx.state.db)
        .await?;
    admin::redirect(&ctx, &format!("admin/user/groups/{id}?ok=1"))
}

// ---------------------------------------------------------------------------
// Permissions

struct Scope {
    module: &'static str,
    title: &'static str,
    icon: &'static str,
    /// Requête listant les objets : `id`, `title`.
    targets: &'static str,
    /// Action, libellé, action de modération (réservée par défaut aux administrateurs).
    actions: &'static [(&'static str, &'static str, bool)],
}

const SCOPES: [Scope; 4] = [
    Scope {
        module: "admin",
        title: "Administration des modules",
        icon: "fas fa-tachometer-alt",
        targets: "",
        actions: &[("admin", "Administrer le module", true)],
    },
    Scope {
        module: "talks",
        title: "Discussions",
        icon: "far fa-comment",
        targets: "SELECT talk_id AS id, name AS title FROM nf_talks WHERE talk_id > 1 ORDER BY talk_id",
        actions: &[("read", "Lire", false), ("write", "Écrire", false), ("delete", "Supprimer un message", true)],
    },
    Scope {
        module: "forum",
        title: "Catégories du forum",
        icon: "fas fa-comments",
        targets: "SELECT category_id AS id, title FROM nf_forum_categories ORDER BY `order`, category_id",
        actions: &[
            ("category_read", "Lire", false),
            ("category_write", "Écrire", false),
            ("category_modify", "Modifier un message", true),
            ("category_delete", "Supprimer un message", true),
            ("category_announce", "Mettre en annonce", true),
            ("category_lock", "Verrouiller un sujet", true),
            ("category_move", "Déplacer un sujet", true),
        ],
    },
    Scope {
        module: "events",
        title: "Types d'événements",
        icon: "far fa-calendar-alt",
        targets: "SELECT type_id AS id, title FROM nf_events_types ORDER BY type_id",
        actions: &[("access_events_type", "Voir les événements", false)],
    },
];

fn scope(module: &str) -> Option<&'static Scope> {
    SCOPES.iter().find(|s| s.module == module)
}

pub async fn access_index(Extension(ctx): Ctxt) -> Reply {
    admin_guard!(ctx);
    let mut content = String::new();
    for scope in &SCOPES {
        let mut body = String::new();
        for (id, title) in targets(&ctx, scope).await? {
            body.push_str(&format!(
                "<tr><td>{title}</td><td class=\"text-right\">{}</td></tr>",
                admin::edit_button(&ctx, &format!("admin/access/{}/{id}", scope.module)),
            ));
        }
        content.push_str(&admin::list_card(&ctx, &format!("{} {}", text::icon(scope.icon), scope.title), None, &["Nom", ""], &body));
    }
    admin::render(&ctx, "access", "Permissions", None, content)
}

/// Objets réglables d'un domaine : identifiant (dans l'URL) et titre.
async fn targets(ctx: &Ctx, scope: &Scope) -> Result<Vec<(u32, String)>, AppError> {
    if scope.module == "admin" {
        // Un « objet » par module délégable, numéroté à partir de 1.
        return Ok(DELEGABLE
            .iter()
            .enumerate()
            .filter(|(_, module)| ctx.enabled(module))
            .map(|(i, module)| {
                let title = MODULE_TITLES.iter().find(|(name, _)| name == module).map(|(_, t)| *t).unwrap_or(module);
                (i as u32 + 1, title.to_owned())
            })
            .collect());
    }
    let mut list = Vec::new();
    for row in sqlx::query(scope.targets).fetch_all(&ctx.state.db).await? {
        list.push((row.try_get("id")?, text::clean(&row.try_get::<String, _>("title")?)));
    }
    Ok(list)
}

async fn target_title(ctx: &Ctx, scope: &Scope, id: u32) -> Result<Option<String>, AppError> {
    Ok(targets(ctx, scope).await?.into_iter().find(|(target, _)| *target == id).map(|(_, title)| title))
}

/// Clé `(module, id)` de `nf_access` pour un objet. L'administration d'un module est
/// enregistrée sous le nom du module, avec l'identifiant 0.
fn access_key(scope: &Scope, id: u32) -> (&'static str, u32) {
    if scope.module == "admin" {
        (DELEGABLE.get(id as usize - 1).copied().unwrap_or("admin"), 0)
    } else {
        (scope.module, id)
    }
}

/// Groupes réglables : groupes personnalisés, puis membres et visiteurs.
/// (Les administrateurs ont toujours tous les droits.)
async fn entities(ctx: &Ctx) -> Result<Vec<(String, String)>, AppError> {
    let mut list: Vec<(String, String)> = groups(ctx).await?.into_iter().map(|g| (g.group_id.to_string(), g.title)).collect();
    list.push(("members".to_owned(), "Membres".to_owned()));
    list.push(("visitors".to_owned(), "Visiteurs".to_owned()));
    Ok(list)
}

pub async fn access_edit(Extension(ctx): Ctxt, Path((module, id)): Path<(String, u32)>, Query(notice): Query<Notice>) -> Reply {
    admin_guard!(ctx);
    let Some(scope) = scope(&module) else {
        return layout::not_found(&ctx).await;
    };
    let Some(title) = target_title(&ctx, scope, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let rows = sqlx::query(
        "SELECT a.action, d.entity, (d.authorized = '1') AS authorized FROM nf_access a \
         INNER JOIN nf_access_details d ON d.access_id = a.access_id \
         WHERE a.module = ? AND a.id = ? AND d.type = 'group'",
    )
    .bind(access_key(scope, id).0)
    .bind(access_key(scope, id).1)
    .fetch_all(&ctx.state.db)
    .await?;
    let mut current: HashMap<(String, String), bool> = HashMap::new();
    for row in &rows {
        current.insert((row.try_get("action")?, row.try_get("entity")?), row.try_get::<i64, _>("authorized")? != 0);
    }
    let entities = entities(&ctx).await?;
    let head: String = entities.iter().map(|(_, title)| format!("<th>{title}</th>")).collect();
    let mut body = String::new();
    for (action, label, restricted) in scope.actions {
        let mut cells = String::new();
        for (entity, _) in &entities {
            let value = current.get(&((*action).to_owned(), entity.clone()));
            let option = |v: &str, label: &str, on: bool| format!("<option value=\"{v}\"{}>{label}</option>", if on { " selected" } else { "" });
            cells.push_str(&format!(
                "<td><select class=\"form-control form-control-sm\" name=\"{action}:{entity}\">{}{}{}</select></td>",
                option("", "Par défaut", value.is_none()),
                option("1", "Autorisé", value == Some(&true)),
                option("0", "Refusé", value == Some(&false)),
            ));
        }
        body.push_str(&format!(
            "<tr><td>{label}{}</td>{cells}</tr>",
            if *restricted { "<br /><small class=\"text-muted\">Modération</small>" } else { "" }
        ));
    }
    let content = format!(
        "<form method=\"post\" action=\"{}\">{}<div class=\"card\"><div class=\"card-body\"><h5>{} — {title}</h5>\
         <p class=\"text-muted mb-0\">« Par défaut » : autorisé si l'action comporte surtout des refus (ou aucune règle), refusé si elle comporte surtout des autorisations. \
         Les actions de modération sont refusées par défaut. Les administrateurs ont toujours tous les droits.</p></div>\
         <div class=\"table-responsive\"><table class=\"table mb-0\"><thead><tr><th>Action</th>{head}</tr></thead><tbody>{body}</tbody></table></div>\
         <div class=\"card-body\"><button type=\"submit\" class=\"btn btn-primary\">Enregistrer</button> <a class=\"btn btn-link\" href=\"{}\">Retour</a></div></div></form>",
        ctx.url(&format!("admin/access/{}/{id}", scope.module)),
        form::csrf_field(&ctx),
        scope.title,
        ctx.url("admin/access"),
    );
    admin::render(&ctx, "access", "Permissions", notice.message(), content)
}

pub async fn access_save(Extension(ctx): Ctxt, Path((module, id)): Path<(String, u32)>, Form(form): Form<HashMap<String, String>>) -> Reply {
    admin_guard!(ctx);
    admin::check_csrf(&ctx, form.get("csrf").map(String::as_str).unwrap_or(""))?;
    let Some(scope) = scope(&module) else {
        return layout::not_found(&ctx).await;
    };
    if target_title(&ctx, scope, id).await?.is_none() {
        return layout::not_found(&ctx).await;
    }
    let entities = entities(&ctx).await?;
    let (key_module, key_id) = access_key(scope, id);
    let mut tx = ctx.state.db.begin().await?;
    for (action, _, restricted) in scope.actions {
        sqlx::query("INSERT IGNORE INTO nf_access (id, module, action) VALUES (?, ?, ?)")
            .bind(key_id)
            .bind(key_module)
            .bind(action)
            .execute(&mut *tx)
            .await?;
        let access_id: u32 = sqlx::query_scalar("SELECT access_id FROM nf_access WHERE id = ? AND module = ? AND action = ?")
            .bind(key_id)
            .bind(key_module)
            .bind(action)
            .fetch_one(&mut *tx)
            .await?;
        // Seules les règles de groupe sont réécrites ; les règles nominatives restent.
        sqlx::query("DELETE FROM nf_access_details WHERE access_id = ? AND type = 'group'")
            .bind(access_id)
            .execute(&mut *tx)
            .await?;
        if *restricted {
            // Une autorisation explicite des administrateurs fait de la règle une liste
            // blanche : sans elle, une action sans règle serait ouverte à tous.
            sqlx::query("INSERT INTO nf_access_details (access_id, entity, type, authorized) VALUES (?, 'admins', 'group', '1')")
                .bind(access_id)
                .execute(&mut *tx)
                .await?;
        }
        for (entity, _) in &entities {
            let authorized = match form.get(&format!("{action}:{entity}")).map(String::as_str) {
                Some("1") => "1",
                Some("0") => "0",
                _ => continue,
            };
            sqlx::query("INSERT INTO nf_access_details (access_id, entity, type, authorized) VALUES (?, ?, 'group', ?)")
                .bind(access_id)
                .bind(entity)
                .bind(authorized)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    admin::redirect(&ctx, &format!("admin/access/{}/{id}?ok=1", scope.module))
}

// ---------------------------------------------------------------------------
// Statistiques

const SERIES: [(&str, &str, &str); 6] = [
    ("Inscriptions", "SELECT DATE_FORMAT(registration_date, '%Y-%m') AS month, COUNT(*) AS total FROM nf_user WHERE deleted = '0' AND registration_date >= ? GROUP BY month", "user"),
    ("Membres connectés", "SELECT DATE_FORMAT(date, '%Y-%m') AS month, COUNT(DISTINCT user_id) AS total FROM nf_session_history WHERE date >= ? GROUP BY month", "user"),
    ("Actualités", "SELECT DATE_FORMAT(date, '%Y-%m') AS month, COUNT(*) AS total FROM nf_news WHERE date >= ? GROUP BY month", "news"),
    ("Commentaires", "SELECT DATE_FORMAT(date, '%Y-%m') AS month, COUNT(*) AS total FROM nf_comment WHERE date >= ? GROUP BY month", "comments"),
    ("Messages du forum", "SELECT DATE_FORMAT(date, '%Y-%m') AS month, COUNT(*) AS total FROM nf_forum_messages WHERE date >= ? GROUP BY month", "forum"),
    ("Candidatures", "SELECT DATE_FORMAT(date, '%Y-%m') AS month, COUNT(*) AS total FROM nf_recruits_candidacies WHERE date >= ? GROUP BY month", "recruits"),
];

/// Les douze derniers mois, du plus ancien au plus récent (`AAAA-MM`).
fn last_months(today: chrono::NaiveDate) -> Vec<String> {
    use chrono::Datelike;
    let (mut year, mut month) = (today.year(), today.month());
    let mut months = Vec::with_capacity(12);
    for _ in 0..12 {
        months.push(format!("{year:04}-{month:02}"));
        if month == 1 {
            year -= 1;
            month = 12;
        } else {
            month -= 1;
        }
    }
    months.reverse();
    months
}

pub async fn statistics(Extension(ctx): Ctxt) -> Reply {
    admin_guard!(ctx);
    let months = last_months(chrono::Local::now().date_naive());
    let since = format!("{}-01 00:00:00", months[0]);
    let head: String = months.iter().map(|m| format!("<th class=\"text-center\">{}/{}</th>", &m[5..], &m[2..4])).collect();
    let mut body = String::new();
    for (title, sql, module) in SERIES {
        if !ctx.enabled(module) {
            continue;
        }
        let mut by_month: HashMap<String, i64> = HashMap::new();
        for row in sqlx::query(sql).bind(&since).fetch_all(&ctx.state.db).await? {
            by_month.insert(row.try_get("month")?, row.try_get("total")?);
        }
        let max = by_month.values().copied().max().unwrap_or(0).max(1);
        let cells: String = months
            .iter()
            .map(|m| {
                let n = by_month.get(m).copied().unwrap_or(0);
                format!(
                    "<td class=\"text-center align-bottom\"><div style=\"height:{}px;background:#00d7b3;margin:0 auto 4px;width:60%\"></div>{n}</td>",
                    n * 40 / max
                )
            })
            .collect();
        let total: i64 = by_month.values().sum();
        body.push_str(&format!("<tr><td class=\"align-bottom\"><b>{title}</b><br /><small class=\"text-muted\">{total} sur 12 mois</small></td>{cells}</tr>"));
    }
    let max_simultaneous: Option<String> = sqlx::query_scalar("SELECT value FROM nf_statistics WHERE name = 'nf_sessions_max_simultaneous'")
        .fetch_optional(&ctx.state.db)
        .await?;
    let online: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nf_session WHERE last_activity > DATE_SUB(NOW(), INTERVAL 5 MINUTE)")
        .fetch_one(&ctx.state.db)
        .await?;
    let content = format!(
        "<div class=\"card\"><div class=\"card-body\"><h5>Douze derniers mois</h5></div>\
         <div class=\"table-responsive\"><table class=\"table mb-0\"><thead><tr><th></th>{head}</tr></thead><tbody>{body}</tbody></table></div></div>\
         <div class=\"card\"><div class=\"card-body\">Sessions actives (5 dernières minutes) : <b>{online}</b> · Record de sessions simultanées : <b>{}</b></div></div>",
        text::encode(&max_simultaneous.unwrap_or_else(|| "0".to_owned())),
    );
    admin::render(&ctx, "statistics", "Statistiques", None, content)
}

// ---------------------------------------------------------------------------
// Supervision

fn dir_size(path: &std::path::Path, depth: u8) -> (u64, u64) {
    let (mut bytes, mut files) = (0, 0);
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            match entry.metadata() {
                Ok(meta) if meta.is_file() => {
                    bytes += meta.len();
                    files += 1;
                }
                Ok(meta) if meta.is_dir() && depth > 0 => {
                    let (b, f) = dir_size(&entry.path(), depth - 1);
                    bytes += b;
                    files += f;
                }
                _ => {}
            }
        }
    }
    (bytes, files)
}

fn human_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} Go", b as f64 / f64::from(1u32 << 30)),
        b if b >= 1 << 20 => format!("{:.1} Mo", b as f64 / f64::from(1u32 << 20)),
        b if b >= 1 << 10 => format!("{:.1} ko", b as f64 / 1024.0),
        b => format!("{b} o"),
    }
}

pub async fn monitoring(Extension(ctx): Ctxt) -> Reply {
    admin_guard!(ctx);
    let db = &ctx.state.db;
    let config = &ctx.state.config;
    let db_version: String = sqlx::query_scalar("SELECT VERSION()").fetch_one(db).await?;
    let db_size: Option<i64> = sqlx::query_scalar(
        "SELECT CAST(SUM(data_length + index_length) AS SIGNED) FROM information_schema.tables WHERE table_schema = DATABASE()",
    )
    .fetch_one(db)
    .await?;
    let tables: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE()")
        .fetch_one(db)
        .await?;
    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nf_session").fetch_one(db).await?;

    let upload_dir = config.site.upload_dir.clone();
    let (upload_ok, (upload_bytes, upload_files)) = tokio::task::spawn_blocking(move || {
        let probe = upload_dir.join(".neofrag-write-test");
        let writable = std::fs::write(&probe, b"").is_ok();
        let _ = std::fs::remove_file(&probe);
        (writable, dir_size(&upload_dir, 4))
    })
    .await
    .map_err(|_| AppError::Internal("supervision interrompue"))?;

    let check = |ok: bool, good: &str, bad: &str| {
        if ok {
            format!("<span class=\"text-success\">{} {good}</span>", text::icon("fas fa-check"))
        } else {
            format!("<span class=\"text-danger\">{} {bad}</span>", text::icon("fas fa-exclamation-triangle"))
        }
    };
    let rows = [
        ("Version de NeoFrag (Rust)", env!("CARGO_PKG_VERSION").to_owned()),
        ("Base de données", text::encode(&db_version)),
        ("Taille de la base", format!("{} · {tables} tables", human_size(db_size.unwrap_or(0).max(0) as u64))),
        ("Sessions enregistrées", sessions.to_string()),
        (
            "Dossier d'envoi",
            format!(
                "{} · {upload_files} fichiers · {}",
                check(upload_ok, "accessible en écriture", "non accessible en écriture"),
                human_size(upload_bytes)
            ),
        ),
        ("Envoi d'emails", check(mail::enabled(&ctx), "configuré", "non configuré (section [email])")),
        ("HTTPS", check(config.server.https, "activé (cookies sécurisés)", "désactivé : à activer en production")),
        ("Inscriptions", check(ctx.settings.bool("nf_registration_status"), "ouvertes", "fermées")),
        ("Maintenance", check(!ctx.settings.bool("nf_maintenance"), "site ouvert", "site en maintenance")),
    ];
    let body: String = rows.iter().map(|(label, value)| format!("<tr><th style=\"width:30%\">{label}</th><td>{value}</td></tr>")).collect();
    let content = format!("<div class=\"card\"><table class=\"table mb-0\"><tbody>{body}</tbody></table></div>");
    admin::render(&ctx, "monitoring", "Supervision", None, content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_flag_is_rewritten_and_other_data_kept() {
        assert!(addon_enabled("a:1:{s:7:\"enabled\";b:1;}"));
        assert!(addon_enabled(""));
        let off = with_enabled("a:2:{s:5:\"order\";i:3;s:7:\"enabled\";b:1;}", false);
        assert_eq!(off, "a:2:{s:5:\"order\";i:3;s:7:\"enabled\";b:0;}");
        assert!(!addon_enabled(&off));
        assert_eq!(with_enabled("", true), "a:1:{s:7:\"enabled\";b:1;}");
    }

    #[test]
    fn twelve_months_ending_now() {
        let months = last_months(chrono::NaiveDate::from_ymd_opt(2026, 2, 11).unwrap());
        assert_eq!(months.len(), 12);
        assert_eq!(months[0], "2025-03");
        assert_eq!(months[11], "2026-02");
    }

    #[test]
    fn sizes_are_readable() {
        assert_eq!(human_size(512), "512 o");
        assert_eq!(human_size(2048), "2.0 ko");
        assert_eq!(human_size(5 << 20), "5.0 Mo");
    }
}
