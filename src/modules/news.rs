//! Module Actualités, porté de `modules/news` (partie publique).

use crate::{
    core::{
        bbcode,
        ctx::Ctx,
        layout::{self, Page},
        pagination::Pagination,
        panel::{Block, Panel},
        text, user,
    },
    error::AppError,
};
use askama::Template;
use axum::{
    extract::Path,
    response::{IntoResponse, Redirect, Response},
    Extension,
};
use chrono::NaiveDateTime;
use sqlx::{mysql::MySqlRow, MySqlPool, Row};
use std::sync::Arc;

pub const TITLE: &str = "Actualités";
pub const ICON: &str = "far fa-file-alt";

#[derive(Debug, Clone)]
pub struct News {
    pub news_id: u32,
    pub category_id: u32,
    pub user_id: Option<u32>,
    pub date: NaiveDateTime,
    pub published: bool,
    pub title: String,
    pub introduction: String,
    pub content: String,
    pub tags: String,
    /// Chemin de l'image (celle de l'actualité, sinon celle de sa catégorie).
    pub image: Option<String>,
    pub category_name: String,
    pub category_title: String,
    pub username: Option<String>,
    pub avatar: Option<String>,
    pub sex: Option<String>,
    pub quote: Option<String>,
}

impl News {
    fn from_row(row: &MySqlRow) -> sqlx::Result<Self> {
        Ok(Self {
            news_id: row.try_get("news_id")?,
            category_id: row.try_get("category_id")?,
            user_id: row.try_get("author_id")?,
            date: row.try_get("date")?,
            published: row.try_get::<i64, _>("is_published")? != 0,
            title: text::clean(&row.try_get::<String, _>("title")?),
            introduction: text::clean(&row.try_get::<String, _>("introduction")?),
            content: text::clean(&row.try_get::<String, _>("content")?),
            tags: text::clean(&row.try_get::<String, _>("tags")?),
            image: row.try_get("image")?,
            category_name: row.try_get("category_name")?,
            category_title: text::clean(&row.try_get::<String, _>("category_title")?),
            username: row.try_get("username")?,
            avatar: row.try_get("avatar")?,
            sex: row.try_get("sex")?,
            quote: row.try_get::<Option<String>, _>("quote")?.map(|q| text::clean(&q)),
        })
    }

    pub fn path(&self) -> String {
        format!("news/{}/{}", self.news_id, text::url_title(&self.title))
    }

    pub fn category_path(&self) -> String {
        format!("news/category/{}/{}", self.category_id, self.category_name)
    }

    pub fn tag_list(&self) -> Vec<&str> {
        self.tags.split(',').map(str::trim).filter(|t| !t.is_empty()).collect()
    }
}

pub enum Filter {
    All,
    Tag(String),
    Category(u32),
}

const SELECT: &str = "SELECT n.news_id, n.category_id, u.id AS author_id, CAST(n.date AS DATETIME) AS date, \
        (n.published = '1') AS is_published, nl.title, nl.introduction, nl.content, nl.tags, \
        f.path AS image, c.name AS category_name, cl.title AS category_title, \
        u.username, fa.path AS avatar, up.sex, up.quote \
    FROM nf_news n \
    INNER JOIN nf_news_lang nl ON n.news_id = nl.news_id AND nl.lang = ? \
    INNER JOIN nf_news_categories c ON n.category_id = c.category_id \
    INNER JOIN nf_news_categories_lang cl ON c.category_id = cl.category_id AND cl.lang = ? \
    LEFT JOIN nf_user u ON n.user_id = u.id AND u.deleted = '0' \
    LEFT JOIN nf_user_profile up ON up.id = u.id \
    LEFT JOIN nf_file fa ON fa.id = up.avatar \
    LEFT JOIN nf_file f ON f.id = IFNULL(n.image_id, c.image_id)";

fn filter_sql(filter: &Filter, only_published: bool) -> String {
    let mut conditions = Vec::new();
    if only_published {
        conditions.push("n.published = '1'");
    }
    match filter {
        Filter::All => {}
        Filter::Tag(_) => conditions.push("FIND_IN_SET(?, nl.tags)"),
        Filter::Category(_) => conditions.push("n.category_id = ?"),
    }
    if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    }
}

/// Liste paginée d'actualités. Renvoie `(actualités de la page, total)`.
pub async fn list(
    db: &MySqlPool,
    lang: &str,
    filter: &Filter,
    only_published: bool,
    limit: u32,
    offset: u32,
) -> sqlx::Result<(Vec<News>, i64)> {
    let conditions = filter_sql(filter, only_published);

    let count_sql = format!(
        "SELECT COUNT(*) FROM nf_news n \
         INNER JOIN nf_news_lang nl ON n.news_id = nl.news_id AND nl.lang = ? \
         INNER JOIN nf_news_categories c ON n.category_id = c.category_id \
         INNER JOIN nf_news_categories_lang cl ON c.category_id = cl.category_id AND cl.lang = ?{conditions}"
    );
    let mut count = sqlx::query_scalar::<_, i64>(&count_sql).bind(lang).bind(lang);
    let list_sql = format!("{SELECT}{conditions} ORDER BY n.date DESC, n.news_id DESC LIMIT ? OFFSET ?");
    let mut query = sqlx::query(&list_sql).bind(lang).bind(lang);
    match filter {
        Filter::All => {}
        Filter::Tag(tag) => {
            count = count.bind(tag);
            query = query.bind(tag);
        }
        Filter::Category(id) => {
            count = count.bind(id);
            query = query.bind(id);
        }
    }

    let total = count.fetch_one(db).await?;
    let rows = query.bind(limit).bind(offset).fetch_all(db).await?;
    let news = rows.iter().map(News::from_row).collect::<sqlx::Result<Vec<_>>>()?;
    Ok((news, total))
}

pub async fn find(db: &MySqlPool, lang: &str, news_id: u32, only_published: bool) -> sqlx::Result<Option<News>> {
    let published = if only_published { " AND n.published = '1'" } else { "" };
    let sql = format!("{SELECT} WHERE n.news_id = ?{published}");
    let row = sqlx::query(&sql).bind(lang).bind(lang).bind(news_id).fetch_optional(db).await?;
    row.as_ref().map(News::from_row).transpose()
}

pub struct Category {
    pub category_id: u32,
    pub name: String,
    pub title: String,
    pub nb_news: i64,
}

pub async fn categories(db: &MySqlPool, lang: &str) -> sqlx::Result<Vec<Category>> {
    let rows = sqlx::query(
        "SELECT c.category_id, c.name, cl.title, COUNT(n.news_id) AS nb_news \
         FROM nf_news_categories c \
         INNER JOIN nf_news_categories_lang cl ON c.category_id = cl.category_id AND cl.lang = ? \
         LEFT JOIN nf_news n ON n.category_id = c.category_id \
         GROUP BY c.category_id, c.name, cl.title ORDER BY cl.title",
    )
    .bind(lang)
    .fetch_all(db)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(Category {
                category_id: row.try_get("category_id")?,
                name: row.try_get("name")?,
                title: text::clean(&row.try_get::<String, _>("title")?),
                nb_news: row.try_get("nb_news")?,
            })
        })
        .collect()
}

#[derive(Template)]
#[template(path = "news/item.html")]
struct ItemTemplate {
    url: String,
    image: Option<String>,
    title: String,
    introduction: String,
    author: String,
    date: String,
    category_url: String,
    category_title: String,
    tags: Vec<(String, String)>,
    has_more: bool,
    share_url: Option<String>,
    comments: i64,
}

#[derive(Template)]
#[template(path = "news/about.html")]
struct AboutTemplate {
    avatar: String,
    author: String,
    quote: String,
    others: Vec<(String, String)>,
}

fn author_html(ctx: &Ctx, news: &News) -> String {
    match (news.user_id, &news.username) {
        (Some(id), Some(username)) => user::link(ctx.base(), id, username),
        _ => "Visiteur".to_owned(),
    }
}

/// Carte d'une actualité. `full` : vue détaillée (texte complet, boutons de partage).
async fn item_html(ctx: &Ctx, news: &News, full: bool) -> Result<String, AppError> {
    let comments = super::comments::count(ctx, "news", news.news_id).await?;
    let introduction = if full && !news.content.is_empty() {
        format!("{}<br /><br />{}", bbcode::render(&news.introduction), bbcode::render(&news.content))
    } else {
        bbcode::render(&news.introduction)
    };
    Ok(ItemTemplate {
        url: ctx.url(&news.path()),
        image: news.image.as_deref().filter(|p| !p.is_empty()).map(|p| ctx.url(p)),
        title: news.title.clone(),
        introduction,
        author: author_html(ctx, news),
        date: text::date_short(&news.date),
        category_url: ctx.url(&news.category_path()),
        category_title: news.category_title.clone(),
        tags: news
            .tag_list()
            .into_iter()
            .map(|tag| (ctx.url(&format!("news/tag/{}", text::url_title(tag))), tag.to_owned()))
            .collect(),
        has_more: !full && !news.content.is_empty(),
        share_url: full.then(|| urlencoding::encode(&ctx.url(&news.path())).into_owned()),
        comments,
    }
    .render()?)
}

fn parse_page(raw: Option<&str>) -> Option<u32> {
    match raw {
        None => Some(1),
        Some(raw) => raw.parse().ok().filter(|p| *p >= 1),
    }
}

/// Liste d'actualités (accueil du module, tag ou catégorie).
async fn listing(
    ctx: &Ctx,
    filter: Filter,
    base_path: &str,
    page: Option<&str>,
    heading: Option<(String, String)>,
    method: &'static str,
) -> Result<Response, AppError> {
    let Some(page_no) = parse_page(page) else {
        return layout::not_found(ctx).await;
    };
    let per_page = match ctx.settings.int("news_per_page") {
        n if n > 0 => n,
        _ => 5,
    };
    let lang = ctx.lang();

    let (_, total) = list(&ctx.state.db, lang, &filter, true, 0, 0).await?;
    let pagination = Pagination::new(page_no, per_page, total);
    if pagination.out_of_range() {
        return Ok(Redirect::to(&ctx.url(base_path)).into_response());
    }
    let (news, _) = list(&ctx.state.db, lang, &filter, true, pagination.per_page, pagination.offset()).await?;

    let mut blocks = Vec::new();
    let mut page = Page::new("news", TITLE, ICON).method(method);
    if let Some((label, value)) = heading {
        page = page.title(format!("{label} {value}"));
        blocks.push(Block::Panel(Panel::new().body(format!(
            "<h3 class=\"m-0\">{label} <small>{value}</small>\
             <a class=\"btn btn-outline-danger float-right\" href=\"{}\" title=\"Voir toutes les actualités\">{}</a></h3>",
            ctx.url("news"),
            text::icon("fas fa-times")
        ))));
    }

    if news.is_empty() {
        blocks.push(Block::Panel(
            Panel::new()
                .heading(TITLE, ICON)
                .style("card-info")
                .body("<div class=\"text-center\">Aucune actualité n'a été publiée pour le moment</div>"),
        ));
    }
    for item in &news {
        blocks.push(Block::Panel(Panel::new().body_raw(item_html(ctx, item, false).await?)));
    }
    if let Some(panel) = pagination.panel(&ctx.url(base_path)) {
        blocks.push(Block::Panel(panel));
    }

    layout::render(ctx, page.blocks(&blocks)).await
}

pub async fn index_page(ctx: &Ctx, page: Option<&str>) -> Result<Response, AppError> {
    // Sur l'accueil, la pagination pointe vers /news/page/N.
    listing(ctx, Filter::All, "news", page, None, "index").await
}

pub async fn index(Extension(ctx): Extension<Arc<Ctx>>) -> Result<Response, AppError> {
    index_page(&ctx, None).await
}

pub async fn index_paged(
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(page): Path<String>,
) -> Result<Response, AppError> {
    index_page(&ctx, Some(&page)).await
}

async fn tag_page(ctx: &Ctx, tag: &str, page: Option<&str>) -> Result<Response, AppError> {
    let slug = text::url_title(tag);
    if slug.is_empty() {
        return layout::not_found(ctx).await;
    }
    let heading = Some(("Tag".to_owned(), text::encode(&slug)));
    listing(ctx, Filter::Tag(slug.clone()), &format!("news/tag/{slug}"), page, heading, "_tag").await
}

pub async fn tag(Extension(ctx): Extension<Arc<Ctx>>, Path(tag): Path<String>) -> Result<Response, AppError> {
    tag_page(&ctx, &tag, None).await
}

pub async fn tag_paged(
    Extension(ctx): Extension<Arc<Ctx>>,
    Path((tag, page)): Path<(String, String)>,
) -> Result<Response, AppError> {
    tag_page(&ctx, &tag, Some(&page)).await
}

async fn category_page(ctx: &Ctx, id: &str, name: &str, page: Option<&str>) -> Result<Response, AppError> {
    let Ok(id) = id.parse::<u32>() else {
        return layout::not_found(ctx).await;
    };
    let category = categories(&ctx.state.db, ctx.lang())
        .await?
        .into_iter()
        .find(|c| c.category_id == id && c.name == name);
    let Some(category) = category else {
        return layout::not_found(ctx).await;
    };
    let base_path = format!("news/category/{}/{}", category.category_id, category.name);
    let heading = Some(("Catégorie".to_owned(), category.title));
    listing(ctx, Filter::Category(id), &base_path, page, heading, "_category").await
}

pub async fn category(
    Extension(ctx): Extension<Arc<Ctx>>,
    Path((id, name)): Path<(String, String)>,
) -> Result<Response, AppError> {
    category_page(&ctx, &id, &name, None).await
}

pub async fn category_paged(
    Extension(ctx): Extension<Arc<Ctx>>,
    Path((id, name, page)): Path<(String, String, String)>,
) -> Result<Response, AppError> {
    category_page(&ctx, &id, &name, Some(&page)).await
}

/// Vue détaillée d'une actualité : `news/{id}/{titre}`.
pub async fn show(
    Extension(ctx): Extension<Arc<Ctx>>,
    Path((id, slug)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let Ok(id) = id.parse::<u32>() else {
        return layout::not_found(&ctx).await;
    };
    let db = &ctx.state.db;
    let news = match find(db, ctx.lang(), id, true).await? {
        Some(news) if text::url_title(&news.title) == slug => news,
        _ => return layout::not_found(&ctx).await,
    };

    sqlx::query("UPDATE nf_news SET views = views + 1 WHERE news_id = ?")
        .bind(id)
        .execute(db)
        .await?;

    let mut blocks = vec![Block::Html(format!(
        "<div class=\"row\"><div class=\"col-12\">{}</div></div>",
        Panel::new().body_raw(item_html(&ctx, &news, true).await?).render()
    ))];

    if let (Some(author_id), Some(username)) = (news.user_id, &news.username) {
        let rows = sqlx::query(
            "SELECT n.news_id, nl.title FROM nf_news n \
             INNER JOIN nf_news_lang nl ON n.news_id = nl.news_id AND nl.lang = ? \
             WHERE n.published = '1' AND n.user_id = ? AND n.news_id <> ? \
             ORDER BY n.date DESC LIMIT 5",
        )
        .bind(ctx.lang())
        .bind(author_id)
        .bind(id)
        .fetch_all(db)
        .await?;
        let mut others = Vec::with_capacity(rows.len());
        for row in rows {
            let other_id: u32 = row.try_get("news_id")?;
            let title: String = row.try_get("title")?;
            others.push((
                ctx.url(&format!("news/{}/{}", other_id, text::url_title(&title))),
                text::shorten(&title, 45),
            ));
        }
        let about = AboutTemplate {
            avatar: user::avatar_html(ctx.base(), news.avatar.as_deref(), news.sex.as_deref(), false, ""),
            author: user::link(ctx.base(), author_id, username),
            quote: news.quote.clone().unwrap_or_default(),
            others,
        }
        .render()?;
        blocks.push(Block::Html(format!(
            "<div class=\"row\"><div class=\"col-12\">{about}</div></div>"
        )));
    }

    blocks.push(Block::Html(format!(
        "<div class=\"row\"><div class=\"col-12\">{}</div></div>",
        super::comments::panel(&ctx, "news", id).await?
    )));

    let page = Page::new("news", TITLE, ICON)
        .method("_news")
        .title(news.title.clone())
        .crumb(news.title.clone(), news.path())
        .blocks(&blocks);
    layout::render(&ctx, page).await
}
