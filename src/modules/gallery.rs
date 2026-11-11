//! Module Galerie : catégories, albums et images. Porté de `modules/gallery` et
//! `widgets/gallery`.
//!
//! Chaque image envoyée est enregistrée en trois fichiers, comme dans l'original :
//! l'original (borné à 4000 px), la version affichée (1600 px) et la vignette (carrée).

use super::{
    admin::{self, admin_guard, Ctxt, CsrfOnly, Notice, Reply},
    comments,
};
use crate::{
    core::{
        bbcode,
        ctx::Ctx,
        files::{self, Fit, FormData},
        form::{self, checkbox, fits, input, textarea},
        layout::{self, Page},
        pagination::Pagination,
        panel::{Block, Panel},
        text,
    },
    error::AppError,
};
use axum::{
    extract::{Multipart, Path, Query},
    response::{IntoResponse, Redirect},
    Extension, Form,
};
use serde::Deserialize;
use sqlx::Row;

pub const TITLE: &str = "Galerie";
pub const ICON: &str = "far fa-image";
const IMAGES_PER_PAGE: i64 = 24;
/// Nombre maximal d'images par envoi.
pub const MAX_IMAGES_PER_UPLOAD: usize = 10;

struct Album {
    gallery_id: u32,
    category_id: u32,
    name: String,
    title: String,
    description: String,
    published: bool,
    cover: Option<String>,
    cover_id: Option<u32>,
    category_title: String,
    category_name: String,
    images: i64,
}

impl Album {
    fn path(&self) -> String {
        format!("gallery/album/{}/{}", self.gallery_id, self.name)
    }
}

/// La couverture est l'image choisie, sinon la vignette de la dernière image de l'album.
const SELECT: &str = "SELECT g.gallery_id, g.category_id, g.name, gl.title, gl.description, (g.published = '1') AS is_published, \
        g.image_id AS cover_id, \
        IFNULL(f.path, (SELECT ft.path FROM nf_gallery_images gi INNER JOIN nf_file ft ON ft.id = gi.thumbnail_file_id \
                         WHERE gi.gallery_id = g.gallery_id ORDER BY gi.image_id DESC LIMIT 1)) AS cover, \
        cl.title AS category_title, c.name AS category_name, \
        (SELECT COUNT(*) FROM nf_gallery_images gi WHERE gi.gallery_id = g.gallery_id) AS images \
     FROM nf_gallery g \
     INNER JOIN nf_gallery_lang gl ON gl.gallery_id = g.gallery_id AND gl.lang = ? \
     INNER JOIN nf_gallery_categories c ON c.category_id = g.category_id \
     INNER JOIN nf_gallery_categories_lang cl ON cl.category_id = c.category_id AND cl.lang = ? \
     LEFT JOIN nf_file f ON f.id = g.image_id";

fn from_row(row: &sqlx::mysql::MySqlRow) -> sqlx::Result<Album> {
    let clean = |name: &str| row.try_get::<String, _>(name).map(|v| text::clean(&v));
    Ok(Album {
        gallery_id: row.try_get("gallery_id")?,
        category_id: row.try_get("category_id")?,
        name: row.try_get("name")?,
        title: clean("title")?,
        description: clean("description")?,
        published: row.try_get::<i64, _>("is_published")? != 0,
        cover: row.try_get("cover")?,
        cover_id: row.try_get("cover_id")?,
        category_title: clean("category_title")?,
        category_name: row.try_get("category_name")?,
        images: row.try_get("images")?,
    })
}

async fn albums(ctx: &Ctx, category: Option<u32>, only_published: bool) -> Result<Vec<Album>, AppError> {
    let mut conditions = Vec::new();
    if only_published {
        conditions.push("g.published = '1'");
    }
    if category.is_some() {
        conditions.push("g.category_id = ?");
    }
    let where_sql = if conditions.is_empty() { String::new() } else { format!(" WHERE {}", conditions.join(" AND ")) };
    let sql = format!("{SELECT}{where_sql} ORDER BY cl.title, g.gallery_id DESC");
    let mut query = sqlx::query(&sql).bind(ctx.lang()).bind(ctx.lang());
    if let Some(category) = category {
        query = query.bind(category);
    }
    let rows = query.fetch_all(&ctx.state.db).await?;
    Ok(rows.iter().map(from_row).collect::<sqlx::Result<_>>()?)
}

async fn find_album(ctx: &Ctx, gallery_id: u32) -> Result<Option<Album>, AppError> {
    let row = sqlx::query(&format!("{SELECT} WHERE g.gallery_id = ?"))
        .bind(ctx.lang())
        .bind(ctx.lang())
        .bind(gallery_id)
        .fetch_optional(&ctx.state.db)
        .await?;
    Ok(row.as_ref().map(from_row).transpose()?)
}

fn album_card(ctx: &Ctx, album: &Album) -> String {
    let cover = match &album.cover {
        Some(path) => format!("<img class=\"card-img-top\" src=\"{}\" alt=\"\" />", text::encode(&ctx.url(path))),
        None => format!("<div class=\"text-center text-muted py-5\" style=\"font-size: 40px;\">{}</div>", text::icon(ICON)),
    };
    format!(
        "<div class=\"col-12 col-sm-6 col-lg-4 mb-4\"><div class=\"card h-100\"><a href=\"{0}\">{cover}</a>\
         <div class=\"card-body\"><h6 class=\"card-title mb-1\"><a href=\"{0}\">{1}</a></h6>\
         <small class=\"text-muted\">{2} — {3} image(s)</small></div></div></div>",
        ctx.url(&album.path()),
        album.title,
        album.category_title,
        album.images,
    )
}

async fn listing(ctx: &Ctx, category: Option<u32>, heading: String, crumb: Option<(String, String)>) -> Reply {
    let albums = albums(ctx, category, true).await?;
    let block = if albums.is_empty() {
        Block::Panel(
            Panel::new()
                .heading(heading, ICON)
                .style("card-info")
                .body("<div class=\"text-center\">Aucun album pour le moment</div>"),
        )
    } else {
        let cards: String = albums.iter().map(|a| album_card(ctx, a)).collect();
        Block::Html(format!("<h4 class=\"mb-4\">{} {heading}</h4><div class=\"row\">{cards}</div>", text::icon(ICON)))
    };
    let mut page = Page::new("gallery", TITLE, ICON);
    if let Some((title, path)) = crumb {
        page = page.title(title.clone()).crumb(title, path);
    }
    layout::render(ctx, page.blocks(&[block])).await
}

struct Image {
    image_id: u32,
    gallery_id: u32,
    title: String,
    description: String,
    file: String,
    original: String,
    views: i64,
}

impl Image {
    fn path(&self) -> String {
        let slug = text::url_title(&self.title);
        format!("gallery/image/{}/{}", self.image_id, if slug.is_empty() { "image".to_owned() } else { slug })
    }
}

/// Image d'un album publié (utilisé aussi par les commentaires).
pub async fn find_visible_image_path(ctx: &Ctx, image_id: u32) -> Result<Option<String>, AppError> {
    Ok(find_image(ctx, image_id).await?.map(|(image, _)| image.path()))
}

async fn find_image(ctx: &Ctx, image_id: u32) -> Result<Option<(Image, Album)>, AppError> {
    let row = sqlx::query(
        "SELECT gi.image_id, gi.gallery_id, gi.title, gi.description, CAST(gi.views AS SIGNED) AS views, f.path AS file, fo.path AS original \
         FROM nf_gallery_images gi INNER JOIN nf_file f ON f.id = gi.file_id INNER JOIN nf_file fo ON fo.id = gi.original_file_id \
         WHERE gi.image_id = ?",
    )
    .bind(image_id)
    .fetch_optional(&ctx.state.db)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let image = Image {
        image_id: row.try_get("image_id")?,
        gallery_id: row.try_get("gallery_id")?,
        title: text::clean(&row.try_get::<String, _>("title")?),
        description: text::clean(&row.try_get::<String, _>("description")?),
        file: row.try_get("file")?,
        original: row.try_get("original")?,
        views: row.try_get("views")?,
    };
    Ok(find_album(ctx, image.gallery_id).await?.filter(|a| a.published).map(|album| (image, album)))
}

async fn album_page(ctx: &Ctx, gallery_id: u32, name: &str, page_no: u32) -> Reply {
    let album = match find_album(ctx, gallery_id).await? {
        Some(album) if album.published && album.name == name => album,
        _ => return layout::not_found(ctx).await,
    };
    let pagination = Pagination::new(page_no, IMAGES_PER_PAGE, album.images);
    if pagination.out_of_range() {
        return Ok(Redirect::to(&ctx.url(&album.path())).into_response());
    }
    let rows = sqlx::query(
        "SELECT gi.image_id, gi.title, f.path AS thumbnail FROM nf_gallery_images gi \
         INNER JOIN nf_file f ON f.id = gi.thumbnail_file_id WHERE gi.gallery_id = ? ORDER BY gi.image_id DESC LIMIT ? OFFSET ?",
    )
    .bind(gallery_id)
    .bind(pagination.per_page)
    .bind(pagination.offset())
    .fetch_all(&ctx.state.db)
    .await?;
    let mut thumbs = String::new();
    for row in &rows {
        let image_id: u32 = row.try_get("image_id")?;
        let title = text::clean(&row.try_get::<String, _>("title")?);
        let slug = text::url_title(&title);
        thumbs.push_str(&format!(
            "<div class=\"col-6 col-md-4 col-lg-3 mb-4\"><a href=\"{}\" title=\"{title}\"><img class=\"img-fluid rounded\" src=\"{}\" alt=\"\" /></a></div>",
            ctx.url(&format!("gallery/image/{image_id}/{}", if slug.is_empty() { "image" } else { &slug })),
            text::encode(&ctx.url(&row.try_get::<String, _>("thumbnail")?)),
        ));
    }
    let description = if album.description.is_empty() { String::new() } else { format!("<p>{}</p>", bbcode::render(&album.description)) };
    let body = if thumbs.is_empty() {
        format!("{description}<div class=\"text-center\">Cet album ne contient pas encore d'image</div>")
    } else {
        format!("{description}<div class=\"row\">{thumbs}</div>")
    };
    let mut blocks = vec![Block::Panel(Panel::new().heading(album.title.clone(), ICON).body(body))];
    if let Some(panel) = pagination.panel(&ctx.url(&album.path())) {
        blocks.push(Block::Panel(panel));
    }
    let page = Page::new("gallery", TITLE, ICON)
        .method("_gallery")
        .title(album.title.clone())
        .crumb(album.category_title.clone(), format!("gallery/{}/{}", album.category_id, album.category_name))
        .crumb(album.title.clone(), album.path())
        .blocks(&blocks);
    layout::render(ctx, page).await
}

async fn image_page(ctx: &Ctx, image_id: u32) -> Reply {
    let Some((image, album)) = find_image(ctx, image_id).await? else {
        return layout::not_found(ctx).await;
    };
    let db = &ctx.state.db;
    sqlx::query("UPDATE nf_gallery_images SET views = views + 1 WHERE image_id = ?")
        .bind(image_id)
        .execute(db)
        .await?;
    // Navigation dans l'album (les plus récentes d'abord, comme la grille).
    let neighbor = |sql: &'static str| sqlx::query_scalar::<_, u32>(sql).bind(image.gallery_id).bind(image_id).fetch_optional(db);
    let previous = neighbor("SELECT image_id FROM nf_gallery_images WHERE gallery_id = ? AND image_id > ? ORDER BY image_id ASC LIMIT 1").await?;
    let next = neighbor("SELECT image_id FROM nf_gallery_images WHERE gallery_id = ? AND image_id < ? ORDER BY image_id DESC LIMIT 1").await?;
    let nav = |id: Option<u32>, label: &str, class: &str| match id {
        Some(id) => format!("<a class=\"btn btn-light {class}\" href=\"{}\">{label}</a>", ctx.url(&format!("gallery/image/{id}/image"))),
        None => String::new(),
    };
    let body = format!(
        "<img class=\"card-img-top\" src=\"{}\" alt=\"\" /><div class=\"card-body\"><div class=\"float-right text-muted\">{} {} vue(s)</div>\
         <h5 class=\"card-title\">{}</h5>{}<div class=\"mt-3\">{}{}<a class=\"btn btn-link\" href=\"{}\" target=\"_blank\" rel=\"noopener\">{} Taille d'origine</a></div></div>",
        text::encode(&ctx.url(&image.file)),
        text::icon("far fa-eye"),
        image.views + 1,
        image.title,
        bbcode::render(&image.description),
        nav(previous, "« Précédente", "mr-2"),
        nav(next, "Suivante »", "mr-2"),
        text::encode(&ctx.url(&image.original)),
        text::icon("fas fa-expand"),
    );
    let mut blocks = vec![Block::Panel(Panel::new().body_raw(body))];
    if ctx.enabled("comments") {
        blocks.push(Block::Html(comments::panel(ctx, "gallery", image_id).await?));
    }
    let title = if image.title.is_empty() { "Image".to_owned() } else { image.title.clone() };
    let page = Page::new("gallery", TITLE, ICON)
        .method("_image")
        .title(title.clone())
        .crumb(album.title.clone(), album.path())
        .crumb(title, image.path())
        .blocks(&blocks);
    layout::render(ctx, page).await
}

pub async fn index(Extension(ctx): Ctxt) -> Reply {
    dispatch(&ctx, "").await
}

pub async fn route(Extension(ctx): Ctxt, Path(rest): Path<String>) -> Reply {
    dispatch(&ctx, &rest).await
}

async fn dispatch(ctx: &Ctx, rest: &str) -> Reply {
    if !ctx.enabled("gallery") {
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
        [] => listing(ctx, None, TITLE.to_owned(), None).await,
        ["album", id, name] => match id.parse::<u32>() {
            Ok(id) => album_page(ctx, id, name, page_no).await,
            Err(_) => layout::not_found(ctx).await,
        },
        ["image", id, _slug] => match id.parse::<u32>() {
            Ok(id) => image_page(ctx, id).await,
            Err(_) => layout::not_found(ctx).await,
        },
        [id, name] => {
            let Ok(id) = id.parse::<u32>() else {
                return layout::not_found(ctx).await;
            };
            let title: Option<String> = sqlx::query_scalar(
                "SELECT cl.title FROM nf_gallery_categories c INNER JOIN nf_gallery_categories_lang cl \
                 ON cl.category_id = c.category_id AND cl.lang = ? WHERE c.category_id = ? AND c.name = ?",
            )
            .bind(ctx.lang())
            .bind(id)
            .bind(name)
            .fetch_optional(&ctx.state.db)
            .await?;
            match title.map(|t| text::clean(&t)) {
                Some(title) => listing(ctx, Some(id), title.clone(), Some((title, format!("gallery/{id}/{name}")))).await,
                None => layout::not_found(ctx).await,
            }
        }
        _ => layout::not_found(ctx).await,
    }
}

/// Widgets : `index` (albums) ; les autres types affichent les dernières images.
pub async fn widget(ctx: &Ctx, kind: &str) -> Result<Vec<Block>, AppError> {
    if kind == "index" {
        let albums = albums(ctx, None, true).await?;
        let panel = Panel::new().heading("Albums", ICON);
        if albums.is_empty() {
            return Ok(vec![Block::Panel(panel.body("Aucun album pour le moment"))]);
        }
        let items: String = albums
            .iter()
            .take(8)
            .map(|a| {
                format!(
                    "<li class=\"list-group-item\"><span class=\"badge badge-secondary float-right\">{}</span><a href=\"{}\">{}</a></li>",
                    a.images,
                    ctx.url(&a.path()),
                    a.title
                )
            })
            .collect();
        return Ok(vec![Block::Panel(
            panel.body_raw(format!("<ul class=\"list-group list-group-flush\">{items}</ul>")),
        )]);
    }
    let rows = sqlx::query(
        "SELECT gi.image_id, f.path FROM nf_gallery_images gi INNER JOIN nf_file f ON f.id = gi.thumbnail_file_id \
         INNER JOIN nf_gallery g ON g.gallery_id = gi.gallery_id AND g.published = '1' ORDER BY gi.image_id DESC LIMIT 6",
    )
    .fetch_all(&ctx.state.db)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut thumbs = String::new();
    for row in &rows {
        thumbs.push_str(&format!(
            "<div class=\"col-4 mb-2 px-1\"><a href=\"{}\"><img class=\"img-fluid rounded\" src=\"{}\" alt=\"\" /></a></div>",
            ctx.url(&format!("gallery/image/{}/image", row.try_get::<u32, _>("image_id")?)),
            text::encode(&ctx.url(&row.try_get::<String, _>("path")?)),
        ));
    }
    Ok(vec![Block::Panel(
        Panel::new().heading("Dernières images", ICON).body(format!("<div class=\"row mx-0\">{thumbs}</div>")),
    )])
}

// ---------------------------------------------------------------------------
// Administration

pub async fn admin_list(Extension(ctx): Ctxt, Query(notice): Query<Notice>) -> Reply {
    admin_guard!(ctx, "gallery");
    let mut body = String::new();
    for a in albums(&ctx, None, false).await? {
        body.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td>{}</tr>",
            a.title,
            a.category_title,
            a.images,
            admin::badge(a.published, "Publié", "Brouillon"),
            admin::row_actions(&ctx, &format!("admin/gallery/{}/{}", a.gallery_id, a.name), &format!("admin/gallery/delete/{}", a.gallery_id), "Supprimer cet album et toutes ses images ?"),
        ));
    }
    let categories = sqlx::query(
        "SELECT c.category_id, cl.title FROM nf_gallery_categories c INNER JOIN nf_gallery_categories_lang cl \
         ON cl.category_id = c.category_id AND cl.lang = ? ORDER BY cl.title",
    )
    .bind(ctx.lang())
    .fetch_all(&ctx.state.db)
    .await?;
    let mut category_rows = String::new();
    for row in &categories {
        category_rows.push_str(&format!(
            "<tr><td>{}</td><td class=\"text-right\">{}</td></tr>",
            text::clean(&row.try_get::<String, _>("title")?),
            admin::delete_button(&ctx, &format!("admin/gallery/categories/delete/{}", row.try_get::<u32, _>("category_id")?), "Supprimer cette catégorie, ses albums et leurs images ?"),
        ));
    }
    let content = format!(
        "<div class=\"row\"><div class=\"col-12 col-xl-8\">{}</div><div class=\"col-12 col-xl-4 mt-4 mt-xl-0\">\
         <div class=\"card\"><div class=\"card-body\"><h5>Catégories</h5><form method=\"post\" action=\"{}\" class=\"form-inline\">{}\
         <input type=\"text\" class=\"form-control mr-2\" name=\"title\" maxlength=\"100\" placeholder=\"Titre\" required />\
         <button type=\"submit\" class=\"btn btn-primary\">Ajouter</button></form></div>\
         <table class=\"table table-hover mb-0\"><tbody>{category_rows}</tbody></table></div></div></div>",
        admin::list_card(&ctx, "Albums", Some("admin/gallery/add"), &["Titre", "Catégorie", "Images", "État", ""], &body),
        ctx.url("admin/gallery/categories/add"),
        form::csrf_field(&ctx),
    );
    admin::render(&ctx, "gallery", TITLE, notice.message(), content)
}

#[derive(Deserialize)]
pub struct TitleForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    title: String,
}

pub async fn admin_category_add(Extension(ctx): Ctxt, Form(form): Form<TitleForm>) -> Reply {
    admin_guard!(ctx, "gallery");
    admin::check_csrf(&ctx, &form.csrf)?;
    let title = form.title.trim();
    if title.is_empty() || text::url_title(title).is_empty() || !fits(title, 100) {
        return Err(AppError::BadRequest("Le titre est obligatoire (100 caractères maximum)."));
    }
    let mut tx = ctx.state.db.begin().await?;
    let id = sqlx::query("INSERT INTO nf_gallery_categories (name) VALUES (?)")
        .bind(text::url_title(title))
        .execute(&mut *tx)
        .await?
        .last_insert_id();
    sqlx::query("INSERT INTO nf_gallery_categories_lang (category_id, lang, title) VALUES (?, ?, ?)")
        .bind(id)
        .bind(ctx.lang())
        .bind(text::encode(title))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    admin::redirect(&ctx, "admin/gallery")
}

/// Supprime un album : ses images (fichiers compris), puis l'album.
async fn delete_album(ctx: &Ctx, gallery_id: u32) -> Result<(), AppError> {
    let db = &ctx.state.db;
    let images: Vec<u32> = sqlx::query_scalar("SELECT image_id FROM nf_gallery_images WHERE gallery_id = ?")
        .bind(gallery_id)
        .fetch_all(db)
        .await?;
    for image_id in images {
        delete_image(ctx, image_id).await?;
    }
    let cover: Option<u32> = sqlx::query_scalar("SELECT image_id FROM nf_gallery WHERE gallery_id = ?")
        .bind(gallery_id)
        .fetch_optional(db)
        .await?
        .flatten();
    sqlx::query("DELETE FROM nf_gallery WHERE gallery_id = ?").bind(gallery_id).execute(db).await?;
    if let Some(cover) = cover {
        files::delete(ctx, cover).await?;
    }
    Ok(())
}

/// Supprime une image : ses trois fichiers (la ligne part par cascade) et ses commentaires.
async fn delete_image(ctx: &Ctx, image_id: u32) -> Result<(), AppError> {
    let db = &ctx.state.db;
    let row = sqlx::query("SELECT thumbnail_file_id, original_file_id, file_id FROM nf_gallery_images WHERE image_id = ?")
        .bind(image_id)
        .fetch_optional(db)
        .await?;
    let Some(row) = row else {
        return Ok(());
    };
    sqlx::query("DELETE FROM nf_comment WHERE module = 'gallery' AND module_id = ?").bind(image_id).execute(db).await?;
    sqlx::query("DELETE FROM nf_gallery_images WHERE image_id = ?").bind(image_id).execute(db).await?;
    for column in ["thumbnail_file_id", "original_file_id", "file_id"] {
        files::delete(ctx, row.try_get::<u32, _>(column)?).await?;
    }
    Ok(())
}

pub async fn admin_category_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx, "gallery");
    admin::check_csrf(&ctx, &form.csrf)?;
    let db = &ctx.state.db;
    let galleries: Vec<u32> = sqlx::query_scalar("SELECT gallery_id FROM nf_gallery WHERE category_id = ?")
        .bind(id)
        .fetch_all(db)
        .await?;
    for gallery_id in galleries {
        delete_album(&ctx, gallery_id).await?;
    }
    sqlx::query("DELETE FROM nf_gallery_categories WHERE category_id = ?").bind(id).execute(db).await?;
    admin::redirect(&ctx, "admin/gallery")
}

struct Values {
    title: String,
    category_id: String,
    description: String,
    published: bool,
}

impl Values {
    fn from_data(data: &FormData) -> Self {
        Self {
            title: data.text("title").trim().to_owned(),
            category_id: data.text("category_id").to_owned(),
            description: data.text("description").trim().to_owned(),
            published: data.checked("published"),
        }
    }
}

async fn album_form(ctx: &Ctx, action: &str, v: &Values, album: Option<&Album>, error: Option<&str>, notice: Option<&str>) -> Reply {
    let db = &ctx.state.db;
    let categories = sqlx::query(
        "SELECT c.category_id, cl.title FROM nf_gallery_categories c INNER JOIN nf_gallery_categories_lang cl \
         ON cl.category_id = c.category_id AND cl.lang = ? ORDER BY cl.title",
    )
    .bind(ctx.lang())
    .fetch_all(db)
    .await?;
    if categories.is_empty() {
        return admin::render(ctx, "gallery", TITLE, None, "<div class=\"alert alert-info\">Créez d'abord une catégorie.</div>".to_owned());
    }
    let mut options: Vec<(String, String)> = Vec::new();
    for row in &categories {
        options.push((row.try_get::<u32, _>("category_id")?.to_string(), text::decode(&row.try_get::<String, _>("title")?)));
    }
    let cover = match album {
        Some(album) if album.cover_id.is_some() => album.cover.as_deref(),
        _ => None,
    };
    let fields = format!(
        "{}{}{}{}{}",
        input("Titre", "title", &v.title, 100, true, ""),
        form::select("Catégorie", "category_id", &v.category_id, options.iter().map(|(a, b)| (a.as_str(), b.as_str()))),
        admin::image_field(ctx, "Couverture", "image", cover, "Facultative : par défaut, la dernière image de l'album."),
        textarea("Description", "description", &v.description, 5, false),
        checkbox("Publié", "published", v.published),
    );
    let mut content = admin::upload_card(ctx, action, error, fields, "admin/gallery");

    if let Some(album) = album {
        let id = album.gallery_id;
        let images = sqlx::query(
            "SELECT gi.image_id, gi.title, f.path FROM nf_gallery_images gi INNER JOIN nf_file f ON f.id = gi.thumbnail_file_id \
             WHERE gi.gallery_id = ? ORDER BY gi.image_id DESC",
        )
        .bind(id)
        .fetch_all(db)
        .await?;
        let mut thumbs = String::new();
        for row in &images {
            thumbs.push_str(&format!(
                "<div class=\"col-6 col-md-3 col-xl-2 mb-3 text-center\"><img class=\"img-fluid rounded mb-1\" src=\"{}\" alt=\"\" /><br />\
                 <small>{}</small><br />{}</div>",
                text::encode(&ctx.url(&row.try_get::<String, _>("path")?)),
                text::clean(&row.try_get::<String, _>("title")?),
                admin::delete_button(ctx, &format!("admin/gallery/images/delete/{}", row.try_get::<u32, _>("image_id")?), "Supprimer cette image ?"),
            ));
        }
        content.push_str(&format!(
            "<div class=\"card mt-4\"><div class=\"card-body\"><h5>Images</h5>\
             <form method=\"post\" action=\"{}\" enctype=\"multipart/form-data\" class=\"mb-4\">{}\
             <input type=\"file\" class=\"form-control-file mb-2\" name=\"images\" accept=\"image/png,image/jpeg,image/gif,image/webp\" multiple required />\
             <small class=\"form-text text-muted mb-2\">Jusqu'à {MAX_IMAGES_PER_UPLOAD} images par envoi. Le titre reprend le nom du fichier.</small>\
             <button type=\"submit\" class=\"btn btn-primary\">Envoyer</button></form><div class=\"row\">{thumbs}</div></div></div>",
            ctx.url(&format!("admin/gallery/{id}/images/add")),
            form::csrf_field(ctx),
        ));
    }
    admin::render(ctx, "gallery", if album.is_some() { "Modifier l'album" } else { "Ajouter un album" }, notice, content)
}

async fn save(ctx: &Ctx, multipart: Multipart, album: Option<Album>) -> Reply {
    let mut data = admin::read_upload_form(ctx, multipart).await?;
    let v = Values::from_data(&data);
    let action = match &album {
        Some(a) => format!("admin/gallery/{}/{}", a.gallery_id, a.name),
        None => "admin/gallery/add".to_owned(),
    };
    if v.title.is_empty() || text::url_title(&v.title).is_empty() || !fits(&v.title, 100) {
        return album_form(ctx, &action, &v, album.as_ref(), Some("Le titre est obligatoire (100 caractères maximum)."), None).await;
    }
    let db = &ctx.state.db;
    let category_id: Option<u32> = match v.category_id.parse::<u32>() {
        Ok(id) => sqlx::query_scalar("SELECT category_id FROM nf_gallery_categories WHERE category_id = ?")
            .bind(id)
            .fetch_optional(db)
            .await?,
        Err(_) => None,
    };
    let Some(category_id) = category_id else {
        return album_form(ctx, &action, &v, album.as_ref(), Some("Catégorie invalide."), None).await;
    };
    let old = album.as_ref().and_then(|a| a.cover_id);
    let image = match admin::image_change(ctx, &mut data, "image", "gallery/covers", Fit::MaxWidth(800), old).await? {
        Ok(change) => change,
        Err(message) => return album_form(ctx, &action, &v, album.as_ref(), Some(message), None).await,
    };
    let name = text::url_title(&v.title);
    let published = if v.published { "1" } else { "0" };
    let mut tx = db.begin().await?;
    let gallery_id = match &album {
        Some(a) => {
            sqlx::query("UPDATE nf_gallery SET category_id = ?, image_id = ?, name = ?, published = ? WHERE gallery_id = ?")
                .bind(category_id)
                .bind(image.value)
                .bind(&name)
                .bind(published)
                .bind(a.gallery_id)
                .execute(&mut *tx)
                .await?;
            a.gallery_id as u64
        }
        None => sqlx::query("INSERT INTO nf_gallery (category_id, image_id, name, published) VALUES (?, ?, ?, ?)")
            .bind(category_id)
            .bind(image.value)
            .bind(&name)
            .bind(published)
            .execute(&mut *tx)
            .await?
            .last_insert_id(),
    };
    sqlx::query(
        "INSERT INTO nf_gallery_lang (gallery_id, lang, title, description) VALUES (?, ?, ?, ?) \
         ON DUPLICATE KEY UPDATE title = VALUES(title), description = VALUES(description)",
    )
    .bind(gallery_id)
    .bind(ctx.lang())
    .bind(text::encode(&v.title))
    .bind(text::encode(&v.description))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    image.finish(ctx).await?;
    if album.is_some() {
        admin::redirect(ctx, "admin/gallery?ok=1")
    } else {
        admin::redirect(ctx, &format!("admin/gallery/{gallery_id}/{name}"))
    }
}

pub async fn admin_add_get(Extension(ctx): Ctxt) -> Reply {
    admin_guard!(ctx, "gallery");
    let v = Values { title: String::new(), category_id: String::new(), description: String::new(), published: true };
    album_form(&ctx, "admin/gallery/add", &v, None, None, None).await
}

pub async fn admin_add_post(Extension(ctx): Ctxt, multipart: Multipart) -> Reply {
    admin_guard!(ctx, "gallery");
    save(&ctx, multipart, None).await
}

pub async fn admin_edit_get(Extension(ctx): Ctxt, Path((id, _name)): Path<(u32, String)>, Query(notice): Query<Notice>) -> Reply {
    admin_guard!(ctx, "gallery");
    let Some(album) = find_album(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let v = Values {
        title: text::decode(&album.title),
        category_id: album.category_id.to_string(),
        description: text::decode(&album.description),
        published: album.published,
    };
    album_form(&ctx, &format!("admin/gallery/{id}/{}", album.name), &v, Some(&album), None, notice.message()).await
}

pub async fn admin_edit_post(Extension(ctx): Ctxt, Path((id, _name)): Path<(u32, String)>, multipart: Multipart) -> Reply {
    admin_guard!(ctx, "gallery");
    let Some(album) = find_album(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    save(&ctx, multipart, Some(album)).await
}

pub async fn admin_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx, "gallery");
    admin::check_csrf(&ctx, &form.csrf)?;
    delete_album(&ctx, id).await?;
    admin::redirect(&ctx, "admin/gallery?ok=1")
}

/// Titre d'une image d'après le nom du fichier envoyé (sans dossier ni extension).
fn title_from_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let stem = base.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(base);
    stem.replace(['_', '-'], " ").trim().chars().take(80).collect()
}

pub async fn admin_images_add(Extension(ctx): Ctxt, Path(id): Path<u32>, multipart: Multipart) -> Reply {
    admin_guard!(ctx, "gallery");
    let mut data = admin::read_upload_form(&ctx, multipart).await?;
    let Some(album) = find_album(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let uploads = data.take_files("images");
    if uploads.len() > MAX_IMAGES_PER_UPLOAD {
        return Err(AppError::BadRequest("Trop d'images dans un seul envoi."));
    }
    for upload in uploads {
        let title = title_from_file_name(&upload.name);
        // Trois variantes ; si l'une échoue, celles déjà écrites sont retirées.
        let mut saved: Vec<u32> = Vec::new();
        let mut failure = None;
        for (dir, fit) in [
            ("gallery/thumbnails", Fit::Square(300)),
            ("gallery/originals", Fit::MaxWidth(4000)),
            ("gallery", Fit::MaxWidth(1600)),
        ] {
            match files::save_image(&ctx, dir, upload.clone(), fit).await? {
                Ok(file_id) => saved.push(file_id),
                Err(message) => {
                    failure = Some(message);
                    break;
                }
            }
        }
        if let Some(message) = failure {
            for file_id in saved {
                files::delete(&ctx, file_id).await?;
            }
            return Err(AppError::BadRequest(message));
        }
        sqlx::query(
            "INSERT INTO nf_gallery_images (thumbnail_file_id, original_file_id, file_id, gallery_id, title, description, views) \
             VALUES (?, ?, ?, ?, ?, '', 0)",
        )
        .bind(saved[0])
        .bind(saved[1])
        .bind(saved[2])
        .bind(id)
        .bind(text::encode(&title))
        .execute(&ctx.state.db)
        .await?;
    }
    admin::redirect(&ctx, &format!("admin/gallery/{id}/{}?ok=1", album.name))
}

pub async fn admin_image_delete(Extension(ctx): Ctxt, Path(image_id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx, "gallery");
    admin::check_csrf(&ctx, &form.csrf)?;
    let gallery_id: Option<u32> = sqlx::query_scalar("SELECT gallery_id FROM nf_gallery_images WHERE image_id = ?")
        .bind(image_id)
        .fetch_optional(&ctx.state.db)
        .await?;
    delete_image(&ctx, image_id).await?;
    match gallery_id {
        Some(gallery_id) => admin::redirect(&ctx, &format!("admin/gallery/{gallery_id}/x")),
        None => admin::redirect(&ctx, "admin/gallery"),
    }
}

#[cfg(test)]
mod tests {
    use super::title_from_file_name;

    #[test]
    fn titles_from_file_names() {
        assert_eq!(title_from_file_name("lan_party-2026.JPG"), "lan party 2026");
        assert_eq!(title_from_file_name("C:\\photos\\finale.png"), "finale");
        assert_eq!(title_from_file_name("sans-extension"), "sans extension");
        assert_eq!(title_from_file_name(""), "");
    }
}
