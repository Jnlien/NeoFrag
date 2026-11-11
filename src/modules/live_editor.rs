//! Live Editor : composition des zones du thème (dispositions, lignes, colonnes, widgets).
//!
//! L'original est un éditeur visuel en glisser-déposer. Cette version fait la même
//! chose avec des formulaires : ajouter, régler, déplacer et retirer des widgets, et
//! créer une disposition propre à une page. Les données restent au format de
//! l'original (`nf_dispositions` et `nf_widgets`, sérialisation PHP).

use super::admin::{self, admin_guard, Ctxt, CsrfOnly, Notice, Reply};
use crate::{
    core::{
        ctx::Ctx,
        form::{self, input, textarea},
        layout,
        php::{self, Value},
        text,
    },
    error::AppError,
};
use axum::{
    extract::{Path, Query},
    Extension, Form,
};
use sqlx::Row;
use std::collections::HashMap;

const THEME: &str = "azuro";
const ZONES: [&str; 8] = ["Haut", "Entête", "Menu", "Slider", "Avant-contenu", "Contenu", "Post-contenu", "Pied de page"];
const ROOT_CLASS: &str = "NF\\NeoFrag\\Libraries\\Array_";
const ROW_CLASS: &str = "NF\\NeoFrag\\Displayables\\Row";
const COL_CLASS: &str = "NF\\NeoFrag\\Displayables\\Col";
const WIDGET_CLASS: &str = "NF\\NeoFrag\\Displayables\\Widget";

const SIZES: [&str; 13] = ["", "col-1", "col-2", "col-3", "col-4", "col-5", "col-6", "col-7", "col-8", "col-9", "col-10", "col-11", "col-12"];
const STYLES: [&str; 9] = [
    "", "card-default", "card-dark", "card-primary", "card-info", "card-success", "card-warning", "card-danger", "card-transparent",
];

/// Widgets proposés : nom, type, libellé, réglages (clé, libellé).
type Setting = (&'static str, &'static str);
const ALIGN: Setting = ("align", "Alignement (text-left, text-center, text-right)");
const SPACING: [Setting; 8] = [
    ("padding_top", "Marge intérieure haute (px)"),
    ("padding_right", "Marge intérieure droite (px)"),
    ("padding_bottom", "Marge intérieure basse (px)"),
    ("padding_left", "Marge intérieure gauche (px)"),
    ("margin_top", "Marge extérieure haute (px)"),
    ("margin_right", "Marge extérieure droite (px)"),
    ("margin_bottom", "Marge extérieure basse (px)"),
    ("margin_left", "Marge extérieure gauche (px)"),
];
const CATALOG: [(&str, &str, &str, &[Setting]); 32] = [
    ("module", "index", "Contenu de la page", &[]),
    ("header", "index", "Entête (logo ou titre)", &[
        ("display", "Affichage (logo ou title)"),
        ALIGN,
        ("title", "Titre"),
        ("description", "Description"),
        ("color_title", "Couleur du titre"),
        ("color_description", "Couleur de la description"),
    ]),
    ("navigation", "index", "Menu horizontal", &[("links", "Liens"), ("panel", "Dans un panneau (1 ou 0)")]),
    ("navigation", "vertical", "Menu vertical", &[("links", "Liens"), ("panel", "Dans un panneau (1 ou 0)")]),
    ("breadcrumb", "index", "Fil d'Ariane", &[]),
    ("copyright", "index", "Copyright", &[]),
    ("html", "index", "Texte (BBCode)", &[("content", "Contenu")]),
    ("html", "html", "HTML libre", &[("content", "Contenu")]),
    ("search", "index", "Recherche", &[ALIGN]),
    ("user", "index", "Espace membre", &[]),
    ("user", "index_mini", "Espace membre (mini)", &[ALIGN]),
    ("members", "index", "Derniers membres", &[]),
    ("members", "online", "Membres en ligne", &[]),
    ("members", "online_mini", "Membres en ligne (mini)", &[ALIGN]),
    ("news", "index", "Dernières actualités", &[]),
    ("news", "categories", "Catégories d'actualités", &[]),
    ("news", "tags", "Mots-clés des actualités", &[]),
    ("forum", "topics", "Forum : derniers sujets", &[]),
    ("forum", "index", "Forum : derniers messages", &[]),
    ("forum", "statistics", "Forum : statistiques", &[]),
    ("gallery", "index", "Galerie : albums", &[]),
    ("gallery", "image", "Galerie : dernières images", &[]),
    ("events", "index", "Prochains événements", &[]),
    ("events", "matches", "Derniers matchs", &[]),
    ("teams", "index", "Équipes", &[]),
    ("partners", "index", "Partenaires", &[("display_style", "Style des logos (light ou dark)")]),
    ("partners", "column", "Partenaires (colonne)", &[("display_style", "Style des logos (light ou dark)")]),
    ("awards", "index", "Palmarès", &[]),
    ("recruits", "index", "Recrutement", &[]),
    ("talks", "index", "Discussion", &[("talk_id", "Numéro de la discussion")]),
    ("about", "index", "À propos de l'équipe", &[
        ("display_panel", "Dans un panneau (oui ou non)"),
        ("display_teamname", "Afficher le nom (oui ou non)"),
        ("display_logo", "Afficher le logo (oui ou non)"),
        ("display_type", "Afficher le type (oui ou non)"),
        ("display_date", "Afficher la date de création (oui ou non)"),
        ("display_biographie", "Afficher la présentation (oui ou non)"),
        ("logo_width", "Largeur du logo (px)"),
    ]),
    ("socials", "index", "Réseaux sociaux", &[
        ("display_panel", "Dans un panneau (oui ou non)"),
        ("social_display", "Disposition (col-12, col-6, col-4, col-3, ul-inline, ul)"),
        ("social_style", "Style (btn btn-social, btn btn-link…)"),
        ("content_display", "Contenu (all, icon ou legend)"),
        ("icon_size", "Taille des icônes (fa-1x à fa-4x)"),
    ]),
];
// Le slider n'a pas de réglage : il est ajouté à part pour garder le tableau lisible.
const SLIDER: (&str, &str, &str, &[Setting]) = ("slider", "index", "Slider", &[]);

fn catalog() -> impl Iterator<Item = &'static (&'static str, &'static str, &'static str, &'static [Setting])> {
    CATALOG.iter().chain(std::iter::once(&SLIDER))
}

fn settings_of(widget: &str, kind: &str) -> Vec<Setting> {
    let mut list: Vec<Setting> = catalog()
        .find(|(w, k, _, _)| *w == widget && *k == kind)
        .or_else(|| catalog().find(|(w, _, _, _)| *w == widget))
        .map(|(_, _, _, settings)| settings.to_vec())
        .unwrap_or_default();
    if matches!(widget, "about" | "socials") {
        list.extend(SPACING);
    }
    list
}

// ---------------------------------------------------------------------------
// Modèle d'une disposition

#[derive(Debug, Clone, PartialEq, Default)]
struct WidgetNode {
    id: i64,
    style: Option<String>,
    size: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
struct ColNode {
    size: Option<String>,
    widgets: Vec<WidgetNode>,
}

#[derive(Debug, Clone, PartialEq, Default)]
struct RowNode {
    style: Option<String>,
    cols: Vec<ColNode>,
}

fn opt(value: &Value, key: &str) -> Option<String> {
    value.get_str(key).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn parse(raw: &str) -> Vec<RowNode> {
    let Ok(root) = php::unserialize(raw) else {
        return Vec::new();
    };
    let class_is = |value: &Value, class: &str| value.class() == Some(class);
    let mut rows = Vec::new();
    for row in root.get("_array").map(Value::items).unwrap_or_default() {
        if !class_is(row, ROW_CLASS) {
            continue;
        }
        let mut cols = Vec::new();
        for col in row.get("_array").map(Value::items).unwrap_or_default() {
            if !class_is(col, COL_CLASS) {
                continue;
            }
            let widgets = col
                .get("_array")
                .map(Value::items)
                .unwrap_or_default()
                .into_iter()
                .filter(|w| class_is(w, WIDGET_CLASS))
                .filter_map(|w| Some(WidgetNode { id: w.get("_widget")?.as_i64()?, style: opt(w, "_style"), size: opt(w, "_size") }))
                .collect();
            cols.push(ColNode { size: opt(col, "_size"), widgets });
        }
        rows.push(RowNode { style: opt(row, "_style"), cols });
    }
    rows
}

fn list(items: Vec<Value>) -> Value {
    Value::Array(items.into_iter().enumerate().map(|(i, v)| (Value::Int(i as i64), v)).collect())
}

fn nullable(value: &Option<String>) -> Value {
    match value {
        Some(s) => Value::Str(s.clone()),
        None => Value::Null,
    }
}

fn dump(rows: &[RowNode]) -> String {
    let rows = rows
        .iter()
        .map(|row| {
            let cols = row
                .cols
                .iter()
                .map(|col| {
                    let widgets = col
                        .widgets
                        .iter()
                        .map(|w| {
                            Value::Object(
                                WIDGET_CLASS.to_owned(),
                                vec![
                                    ("_widget".to_owned(), Value::Int(w.id)),
                                    ("_style".to_owned(), nullable(&w.style)),
                                    ("_size".to_owned(), nullable(&w.size)),
                                ],
                            )
                        })
                        .collect();
                    Value::Object(COL_CLASS.to_owned(), vec![("_array".to_owned(), list(widgets)), ("_size".to_owned(), nullable(&col.size))])
                })
                .collect();
            Value::Object(ROW_CLASS.to_owned(), vec![("_array".to_owned(), list(cols)), ("_style".to_owned(), nullable(&row.style))])
        })
        .collect();
    php::serialize(&Value::Object(ROOT_CLASS.to_owned(), vec![("_array".to_owned(), list(rows))]))
}

/// Retire un widget ; les colonnes et lignes devenues vides disparaissent.
fn remove_widget(rows: &mut Vec<RowNode>, widget_id: i64) -> bool {
    let mut found = false;
    for row in rows.iter_mut() {
        for col in &mut row.cols {
            let before = col.widgets.len();
            col.widgets.retain(|w| w.id != widget_id);
            found |= col.widgets.len() != before;
        }
        row.cols.retain(|col| !col.widgets.is_empty());
    }
    rows.retain(|row| !row.cols.is_empty());
    found
}

/// Place un widget : `new` (nouvelle ligne), `rN` (nouvelle colonne de la ligne N) ou
/// `rNcM` (sous les widgets de la colonne M de la ligne N).
fn place_widget(rows: &mut Vec<RowNode>, position: &str, widget: WidgetNode) -> bool {
    let target = position.strip_prefix('r').map(|rest| match rest.split_once('c') {
        Some((r, c)) => (r.parse::<usize>().ok(), c.parse::<usize>().ok().map(Some)),
        None => (rest.parse::<usize>().ok(), Some(None)),
    });
    match target {
        None if position == "new" => rows.push(RowNode { style: None, cols: vec![ColNode { size: None, widgets: vec![widget] }] }),
        Some((Some(r), Some(col))) => {
            let Some(row) = rows.get_mut(r) else {
                return false;
            };
            match col {
                None => row.cols.push(ColNode { size: None, widgets: vec![widget] }),
                Some(c) => match row.cols.get_mut(c) {
                    Some(col) => col.widgets.push(widget),
                    None => return false,
                },
            }
        }
        _ => return false,
    }
    true
}

struct Disposition {
    page: String,
    rows: Vec<RowNode>,
}

async fn load(ctx: &Ctx, id: u32) -> Result<Option<Disposition>, AppError> {
    let row = sqlx::query("SELECT page, disposition FROM nf_dispositions WHERE disposition_id = ? AND theme = ?")
        .bind(id)
        .bind(THEME)
        .fetch_optional(&ctx.state.db)
        .await?;
    Ok(match row {
        Some(row) => Some(Disposition {
            page: row.try_get("page")?,
            rows: parse(&row.try_get::<String, _>("disposition")?),
        }),
        None => None,
    })
}

async fn store(ctx: &Ctx, id: u32, rows: &[RowNode]) -> Result<(), AppError> {
    sqlx::query("UPDATE nf_dispositions SET disposition = ? WHERE disposition_id = ?")
        .bind(dump(rows))
        .bind(id)
        .execute(&ctx.state.db)
        .await?;
    Ok(())
}

fn back(ctx: &Ctx, ok: bool) -> Reply {
    admin::redirect(ctx, if ok { "admin/live-editor?ok=1" } else { "admin/live-editor" })
}

// ---------------------------------------------------------------------------
// Vue d'ensemble

fn page_label(page: &str) -> String {
    match page {
        "*" => "Toutes les pages".to_owned(),
        "/" => "Page d'accueil".to_owned(),
        other => format!("Pages <code>{}</code>", text::encode(other)),
    }
}

fn options(values: &[&str], selected: &str, empty: &str) -> String {
    values
        .iter()
        .map(|v| {
            format!(
                "<option value=\"{v}\"{}>{}</option>",
                if *v == selected { " selected" } else { "" },
                if v.is_empty() { empty } else { v }
            )
        })
        .collect()
}

pub async fn index(Extension(ctx): Ctxt, Query(notice): Query<Notice>) -> Reply {
    admin_guard!(ctx);
    let db = &ctx.state.db;
    let mut widgets: HashMap<i64, (String, String, String)> = HashMap::new();
    for row in sqlx::query("SELECT widget_id, widget, type, IFNULL(title, '') AS title FROM nf_widgets").fetch_all(db).await? {
        widgets.insert(
            i64::from(row.try_get::<u32, _>("widget_id")?),
            (row.try_get("widget")?, row.try_get("type")?, text::clean(&row.try_get::<String, _>("title")?)),
        );
    }
    let rows = sqlx::query(
        "SELECT disposition_id, page, zone, disposition FROM nf_dispositions WHERE theme = ? \
         ORDER BY (page = '*') DESC, (page = '/') DESC, page, zone",
    )
    .bind(THEME)
    .fetch_all(db)
    .await?;

    let catalog_options: String = catalog()
        .map(|(widget, kind, label, _)| format!("<option value=\"{widget}/{kind}\">{label}</option>"))
        .collect();
    let csrf = form::csrf_field(&ctx);
    let mut content = String::new();
    let mut current_page = String::new();
    for row in &rows {
        let id: u32 = row.try_get("disposition_id")?;
        let page: String = row.try_get("page")?;
        let zone = row.try_get::<u32, _>("zone")? as usize;
        let tree = parse(&row.try_get::<String, _>("disposition")?);
        if page != current_page {
            content.push_str(&format!("<h5 class=\"mt-4 mb-3\">{}</h5>", page_label(&page)));
            current_page = page.clone();
        }

        let mut positions = String::from("<option value=\"new\">Nouvelle ligne</option>");
        let mut body = String::new();
        for (r, node) in tree.iter().enumerate() {
            positions.push_str(&format!("<option value=\"r{r}\">Ligne {} : nouvelle colonne</option>", r + 1));
            let mut cols = String::new();
            for (c, col) in node.cols.iter().enumerate() {
                positions.push_str(&format!("<option value=\"r{r}c{c}\">Ligne {}, colonne {} : à la suite</option>", r + 1, c + 1));
                let mut items = String::new();
                for w in &col.widgets {
                    let (name, kind, title) = widgets.get(&w.id).cloned().unwrap_or_else(|| ("?".to_owned(), String::new(), String::new()));
                    let label = catalog().find(|(n, k, _, _)| *n == name && *k == kind).map(|(_, _, l, _)| *l).unwrap_or("");
                    items.push_str(&format!(
                        "<div class=\"border rounded p-2 mb-2 bg-white\"><div class=\"float-right text-nowrap\">{} {}</div>\
                         <b>{}</b> <small class=\"text-muted\">{}/{}</small>{}{}</div>",
                        admin::edit_button(&ctx, &format!("admin/live-editor/{id}/widgets/{}", w.id)),
                        admin::delete_button(&ctx, &format!("admin/live-editor/{id}/widgets/delete/{}", w.id), "Retirer ce widget ?"),
                        if label.is_empty() { text::encode(&name) } else { label.to_owned() },
                        text::encode(&name),
                        text::encode(&kind),
                        if title.is_empty() { String::new() } else { format!("<br />« {title} »") },
                        match (&w.size, &w.style) {
                            (None, None) => String::new(),
                            (size, style) => format!(
                                "<br /><small class=\"text-muted\">{} {}</small>",
                                text::encode(size.as_deref().unwrap_or("")),
                                text::encode(style.as_deref().unwrap_or(""))
                            ),
                        },
                    ));
                }
                cols.push_str(&format!("<div class=\"col\">{items}</div>"));
            }
            let mover = |direction: &str, icon: &str, disabled: bool| {
                format!(
                    "<form method=\"post\" action=\"{}\" class=\"d-inline\">{csrf}<button type=\"submit\" class=\"btn btn-sm btn-light\"{}>{}</button></form>",
                    ctx.url(&format!("admin/live-editor/{id}/rows/{r}/{direction}")),
                    if disabled { " disabled" } else { "" },
                    text::icon(icon),
                )
            };
            body.push_str(&format!(
                "<div class=\"d-flex mb-2\"><div class=\"mr-2 text-nowrap\">{}{}</div><div class=\"row flex-grow-1 no-gutters\" style=\"gap:.5rem\">{cols}</div></div>",
                mover("up", "fas fa-arrow-up", r == 0),
                mover("down", "fas fa-arrow-down", r + 1 == tree.len()),
            ));
        }
        if tree.is_empty() {
            body.push_str("<p class=\"text-muted\">Zone vide.</p>");
        }
        let remove = if page == "*" {
            String::new()
        } else {
            format!(
                "<div class=\"float-right\">{}</div>",
                admin::delete_button(&ctx, &format!("admin/live-editor/dispositions/delete/{id}"), "Supprimer cette disposition et ses widgets ?")
            )
        };
        content.push_str(&format!(
            "<div class=\"card\"><div class=\"card-body\">{remove}<h6>Zone {zone} — {}</h6>{body}\
             <form method=\"post\" action=\"{}\" class=\"form-inline mt-3\">{csrf}\
             <select class=\"form-control form-control-sm mr-2 mb-1\" name=\"widget\">{catalog_options}</select>\
             <select class=\"form-control form-control-sm mr-2 mb-1\" name=\"position\">{positions}</select>\
             <select class=\"form-control form-control-sm mr-2 mb-1\" name=\"size\">{}</select>\
             <button type=\"submit\" class=\"btn btn-sm btn-primary mb-1\">{} Ajouter un widget</button></form></div></div>",
            ZONES.get(zone).copied().unwrap_or("?"),
            ctx.url(&format!("admin/live-editor/{id}/widgets/add")),
            options(&SIZES, "", "Largeur automatique"),
            text::icon("fas fa-plus"),
        ));
    }

    let zone_options: String = ZONES.iter().enumerate().map(|(i, name)| format!("<option value=\"{i}\">{i} — {name}</option>")).collect();
    content.push_str(&format!(
        "<div class=\"card mt-4\"><div class=\"card-body\"><h5>Disposition propre à une page</h5>\
         <p class=\"text-muted\">Une zone définie pour une page remplace, sur cette page, la zone de « Toutes les pages ». \
         Exemples : <code>/</code> (accueil), <code>forum/*</code>, <code>news/_news/*</code>.</p>\
         <form method=\"post\" action=\"{}\" class=\"form-inline\">{csrf}\
         <input type=\"text\" class=\"form-control mr-2\" name=\"page\" placeholder=\"forum/*\" maxlength=\"100\" required />\
         <select class=\"form-control mr-2\" name=\"zone\">{zone_options}</select>\
         <button type=\"submit\" class=\"btn btn-primary\">Créer</button></form></div></div>",
        ctx.url("admin/live-editor/dispositions/add"),
    ));
    admin::render(&ctx, "live_editor", "Live Editor", notice.message(), content)
}

// ---------------------------------------------------------------------------
// Dispositions

/// `/`, `module/*` ou `module/methode/*`.
fn valid_page(page: &str) -> bool {
    if page == "/" {
        return true;
    }
    let Some(prefix) = page.strip_suffix("/*") else {
        return false;
    };
    let parts: Vec<&str> = prefix.split('/').collect();
    page.len() <= 100
        && (1..=2).contains(&parts.len())
        && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
}

pub async fn disposition_add(Extension(ctx): Ctxt, Form(form): Form<HashMap<String, String>>) -> Reply {
    admin_guard!(ctx);
    admin::check_csrf(&ctx, form.get("csrf").map(String::as_str).unwrap_or(""))?;
    let page = form.get("page").map(|p| p.trim()).unwrap_or("");
    let zone = form.get("zone").and_then(|z| z.parse::<u32>().ok()).filter(|z| (*z as usize) < ZONES.len());
    let (true, Some(zone)) = (valid_page(page), zone) else {
        return Err(AppError::BadRequest("Page ou zone invalide (exemples : « / », « forum/* », « news/_news/* »)."));
    };
    sqlx::query("INSERT IGNORE INTO nf_dispositions (theme, page, zone, disposition) VALUES (?, ?, ?, ?)")
        .bind(THEME)
        .bind(page)
        .bind(zone)
        .bind(dump(&[]))
        .execute(&ctx.state.db)
        .await?;
    back(&ctx, true)
}

pub async fn disposition_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx);
    admin::check_csrf(&ctx, &form.csrf)?;
    let Some(disposition) = load(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    if disposition.page == "*" {
        return layout::forbidden(&ctx).await;
    }
    let mut tx = ctx.state.db.begin().await?;
    for widget in disposition.rows.iter().flat_map(|r| &r.cols).flat_map(|c| &c.widgets) {
        sqlx::query("DELETE FROM nf_widgets WHERE widget_id = ?").bind(widget.id).execute(&mut *tx).await?;
    }
    sqlx::query("DELETE FROM nf_dispositions WHERE disposition_id = ?").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    back(&ctx, true)
}

pub async fn row_move(Extension(ctx): Ctxt, Path((id, index, direction)): Path<(u32, usize, String)>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx);
    admin::check_csrf(&ctx, &form.csrf)?;
    let Some(mut disposition) = load(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let other = match direction.as_str() {
        "up" => index.checked_sub(1),
        "down" => index.checked_add(1),
        _ => None,
    };
    match other {
        Some(other) if index < disposition.rows.len() && other < disposition.rows.len() => {
            disposition.rows.swap(index, other);
            store(&ctx, id, &disposition.rows).await?;
            back(&ctx, true)
        }
        _ => back(&ctx, false),
    }
}

// ---------------------------------------------------------------------------
// Widgets

fn pick<'a>(allowed: &[&'a str], value: &str) -> Option<String> {
    allowed.iter().copied().find(|a| *a == value && !a.is_empty()).map(str::to_owned)
}

pub async fn widget_add(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<HashMap<String, String>>) -> Reply {
    admin_guard!(ctx);
    admin::check_csrf(&ctx, form.get("csrf").map(String::as_str).unwrap_or(""))?;
    let Some(mut disposition) = load(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let get = |name: &str| form.get(name).map(String::as_str).unwrap_or("");
    let chosen = get("widget").split_once('/').and_then(|(w, k)| catalog().find(|(name, kind, _, _)| *name == w && *kind == k));
    let Some((widget, kind, _, _)) = chosen else {
        return Err(AppError::BadRequest("Widget inconnu."));
    };
    let mut tx = ctx.state.db.begin().await?;
    let widget_id = sqlx::query("INSERT INTO nf_widgets (widget, type) VALUES (?, ?)")
        .bind(widget)
        .bind(kind)
        .execute(&mut *tx)
        .await?
        .last_insert_id();
    let node = WidgetNode { id: widget_id as i64, style: None, size: pick(&SIZES, get("size")) };
    if !place_widget(&mut disposition.rows, get("position"), node) {
        // Transaction abandonnée : le widget créé n'est pas conservé.
        return Err(AppError::BadRequest("Emplacement invalide."));
    }
    sqlx::query("UPDATE nf_dispositions SET disposition = ? WHERE disposition_id = ?")
        .bind(dump(&disposition.rows))
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    admin::redirect(&ctx, &format!("admin/live-editor/{id}/widgets/{widget_id}"))
}

pub async fn widget_delete(Extension(ctx): Ctxt, Path((id, widget_id)): Path<(u32, u32)>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx);
    admin::check_csrf(&ctx, &form.csrf)?;
    let Some(mut disposition) = load(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let name: Option<String> = sqlx::query_scalar("SELECT widget FROM nf_widgets WHERE widget_id = ?")
        .bind(widget_id)
        .fetch_optional(&ctx.state.db)
        .await?;
    if disposition.page == "*" && name.as_deref() == Some("module") {
        return Err(AppError::BadRequest("Ce widget affiche le contenu des pages : il ne peut pas être retiré de « Toutes les pages »."));
    }
    if !remove_widget(&mut disposition.rows, i64::from(widget_id)) {
        return layout::not_found(&ctx).await;
    }
    store(&ctx, id, &disposition.rows).await?;
    sqlx::query("DELETE FROM nf_widgets WHERE widget_id = ?").bind(widget_id).execute(&ctx.state.db).await?;
    back(&ctx, true)
}

/// Liens d'un menu : une ligne « Titre | adresse » par lien.
fn links_to_text(value: Option<&Value>) -> String {
    value
        .map(Value::items)
        .unwrap_or_default()
        .iter()
        .map(|link| format!("{} | {}", text::decode(link.get_str("title").unwrap_or("")), text::decode(link.get_str("url").unwrap_or(""))))
        .collect::<Vec<_>>()
        .join("\n")
}

fn text_to_links(raw: &str) -> Value {
    let links = raw
        .lines()
        .filter_map(|line| {
            let (title, url) = line.split_once('|').unwrap_or((line, ""));
            let (title, url) = (title.trim(), url.trim());
            // Adresse : chemin du site, lien web, ancre ou mailto (jamais `javascript:`).
            let safe = !url.contains(':') || url.starts_with("https://") || url.starts_with("http://") || url.starts_with("mailto:");
            (!title.is_empty() && safe).then(|| {
                Value::Array(vec![
                    (Value::Str("title".to_owned()), Value::Str(text::encode(title))),
                    (Value::Str("url".to_owned()), Value::Str(text::encode(url))),
                ])
            })
        })
        .collect();
    list(links)
}

struct Placed<'a> {
    node: &'a mut WidgetNode,
}

fn find_node(rows: &mut [RowNode], widget_id: i64) -> Option<Placed<'_>> {
    rows.iter_mut()
        .flat_map(|r| r.cols.iter_mut())
        .flat_map(|c| c.widgets.iter_mut())
        .find(|w| w.id == widget_id)
        .map(|node| Placed { node })
}

async fn widget_row(ctx: &Ctx, widget_id: u32) -> Result<Option<(String, String, String, Value)>, AppError> {
    let row = sqlx::query("SELECT widget, type, IFNULL(title, '') AS title, IFNULL(settings, '') AS settings FROM nf_widgets WHERE widget_id = ?")
        .bind(widget_id)
        .fetch_optional(&ctx.state.db)
        .await?;
    Ok(match row {
        Some(row) => {
            let settings = php::unserialize(&row.try_get::<String, _>("settings")?)
                .ok()
                .filter(|v| matches!(v, Value::Array(_)))
                .unwrap_or(Value::Array(Vec::new()));
            Some((row.try_get("widget")?, row.try_get("type")?, row.try_get("title")?, settings))
        }
        None => None,
    })
}

pub async fn widget_edit_get(Extension(ctx): Ctxt, Path((id, widget_id)): Path<(u32, u32)>) -> Reply {
    admin_guard!(ctx);
    let Some(mut disposition) = load(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let (Some(placed), Some((name, kind, title, settings))) = (find_node(&mut disposition.rows, i64::from(widget_id)), widget_row(&ctx, widget_id).await?) else {
        return layout::not_found(&ctx).await;
    };
    let select = |label: &str, field: &str, values: &[&str], selected: &str, empty: &str| {
        format!(
            "<div class=\"form-group\"><label for=\"f-{field}\"><b>{label}</b></label><select class=\"form-control\" id=\"f-{field}\" name=\"{field}\">{}</select></div>",
            options(values, selected, empty)
        )
    };
    let mut fields = format!(
        "{}{}{}",
        input("Titre du panneau", "title", &text::decode(&title), 100, false, "Vide : titre par défaut du widget."),
        select("Largeur", "size", &SIZES, placed.node.size.as_deref().unwrap_or(""), "Automatique"),
        select("Style du panneau", "style", &STYLES, placed.node.style.as_deref().unwrap_or(""), "Par défaut"),
    );
    for (key, label) in settings_of(&name, &kind) {
        let field = format!("s_{key}");
        fields.push_str(&match key {
            "links" => format!(
                "{}<p class=\"text-muted\"><small>Un lien par ligne : <code>Titre | adresse</code> (ex. <code>Forum | forum</code>).</small></p>",
                textarea(label, &field, &links_to_text(settings.get("links")), 8, false)
            ),
            "content" => textarea(label, &field, &text::decode(settings.get_str("content").unwrap_or("")), 10, false),
            _ => {
                let value = match settings.get(key) {
                    Some(Value::Str(s)) => text::decode(s),
                    Some(Value::Int(i)) => i.to_string(),
                    Some(Value::Bool(b)) => (*b as u8).to_string(),
                    _ => String::new(),
                };
                input(label, &field, &value, 200, false, "")
            }
        });
    }
    let label = catalog().find(|(n, k, _, _)| *n == name && *k == kind).map(|(_, _, l, _)| *l).unwrap_or("Widget");
    let content = admin::form_card(&ctx, &format!("admin/live-editor/{id}/widgets/{widget_id}"), None, fields, "admin/live-editor");
    admin::render(&ctx, "live_editor", &format!("Widget — {label}"), None, content)
}

pub async fn widget_edit_post(Extension(ctx): Ctxt, Path((id, widget_id)): Path<(u32, u32)>, Form(form): Form<HashMap<String, String>>) -> Reply {
    admin_guard!(ctx);
    admin::check_csrf(&ctx, form.get("csrf").map(String::as_str).unwrap_or(""))?;
    let Some(mut disposition) = load(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let Some((name, kind, _, old)) = widget_row(&ctx, widget_id).await? else {
        return layout::not_found(&ctx).await;
    };
    let get = |field: &str| form.get(field).map(|v| v.trim()).unwrap_or("");
    {
        let Some(placed) = find_node(&mut disposition.rows, i64::from(widget_id)) else {
            return layout::not_found(&ctx).await;
        };
        placed.node.size = pick(&SIZES, get("size"));
        placed.node.style = pick(&STYLES, get("style"));
    }

    // Les réglages connus sont réécrits ; les autres clés déjà présentes sont conservées.
    let known = settings_of(&name, &kind);
    let mut pairs: Vec<(Value, Value)> = match old {
        Value::Array(pairs) => pairs.into_iter().filter(|(k, _)| !known.iter().any(|(key, _)| k.as_str() == Some(key))).collect(),
        _ => Vec::new(),
    };
    for (key, _) in &known {
        let raw = get(&format!("s_{key}"));
        let value = match *key {
            "links" => text_to_links(raw),
            _ if raw.is_empty() => continue,
            "talk_id" => match raw.parse::<i64>() {
                Ok(n) if n > 0 => Value::Int(n),
                _ => continue,
            },
            "content" => Value::Str(text::encode(&raw.chars().take(50_000).collect::<String>())),
            _ => Value::Str(text::encode(&raw.chars().take(200).collect::<String>())),
        };
        pairs.push((Value::Str((*key).to_owned()), value));
    }
    let settings = if pairs.is_empty() { None } else { Some(php::serialize(&Value::Array(pairs))) };
    let title: String = get("title").chars().take(100).collect();

    let mut tx = ctx.state.db.begin().await?;
    sqlx::query("UPDATE nf_widgets SET title = ?, settings = ? WHERE widget_id = ?")
        .bind(if title.is_empty() { None } else { Some(text::encode(&title)) })
        .bind(settings)
        .bind(widget_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE nf_dispositions SET disposition = ? WHERE disposition_id = ?")
        .bind(dump(&disposition.rows))
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    back(&ctx, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "O:27:\"NF\\NeoFrag\\Libraries\\Array_\":1:{s:9:\"\0*\0_array\";a:1:{i:0;O:27:\"NF\\NeoFrag\\Displayables\\Row\":2:{s:9:\"\0*\0_array\";a:2:{i:0;O:27:\"NF\\NeoFrag\\Displayables\\Col\":2:{s:9:\"\0*\0_array\";a:1:{i:0;O:30:\"NF\\NeoFrag\\Displayables\\Widget\":3:{s:10:\"\0*\0_widget\";i:7;s:9:\"\0*\0_style\";N;s:8:\"\0*\0_size\";s:5:\"col-8\";}}s:8:\"\0*\0_size\";N;}i:1;O:27:\"NF\\NeoFrag\\Displayables\\Col\":2:{s:9:\"\0*\0_array\";a:2:{i:0;O:30:\"NF\\NeoFrag\\Displayables\\Widget\":3:{s:10:\"\0*\0_widget\";i:8;s:9:\"\0*\0_style\";s:9:\"card-dark\";s:8:\"\0*\0_size\";N;}i:1;O:30:\"NF\\NeoFrag\\Displayables\\Widget\":3:{s:10:\"\0*\0_widget\";i:9;s:9:\"\0*\0_style\";N;s:8:\"\0*\0_size\";N;}}s:8:\"\0*\0_size\";s:5:\"col-4\";}}s:9:\"\0*\0_style\";N;}}}";

    #[test]
    fn original_dispositions_round_trip_byte_for_byte() {
        let rows = parse(SAMPLE);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cols.len(), 2);
        assert_eq!(rows[0].cols[1].size.as_deref(), Some("col-4"));
        assert_eq!(rows[0].cols[1].widgets[0].style.as_deref(), Some("card-dark"));
        assert_eq!(dump(&rows), SAMPLE);
        assert_eq!(dump(&[]), "O:27:\"NF\\NeoFrag\\Libraries\\Array_\":1:{s:9:\"\0*\0_array\";a:0:{}}");
    }

    #[test]
    fn widgets_are_placed_and_removed() {
        let mut rows = parse(SAMPLE);
        let w = |id| WidgetNode { id, ..WidgetNode::default() };
        assert!(place_widget(&mut rows, "new", w(20)));
        assert!(place_widget(&mut rows, "r0", w(21)));
        assert!(place_widget(&mut rows, "r0c1", w(22)));
        assert!(!place_widget(&mut rows, "r9", w(23)));
        assert!(!place_widget(&mut rows, "r0c9", w(23)));
        assert!(!place_widget(&mut rows, "x", w(23)));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].cols.len(), 3);
        assert_eq!(rows[0].cols[1].widgets.len(), 3);

        assert!(remove_widget(&mut rows, 20));
        assert_eq!(rows.len(), 1, "la ligne vide disparaît");
        assert!(remove_widget(&mut rows, 7));
        assert_eq!(rows[0].cols.len(), 2, "la colonne vide disparaît");
        assert!(!remove_widget(&mut rows, 999));
    }

    #[test]
    fn page_patterns() {
        for ok in ["/", "forum/*", "news/_news/*", "user/*"] {
            assert!(valid_page(ok), "{ok}");
        }
        for bad in ["*", "", "forum", "forum/", "a/b/c/*", "Forum/*", "../*", "a//*"] {
            assert!(!valid_page(bad), "{bad}");
        }
    }

    #[test]
    fn menu_links_reject_script_urls() {
        let links = text_to_links("Accueil | \nForum | forum\nPiège | javascript:alert(1)\nSite | https://example.com\n | vide");
        assert_eq!(links.items().len(), 3);
        assert_eq!(links_to_text(Some(&links)), "Accueil | \nForum | forum\nSite | https://example.com");
    }
}
