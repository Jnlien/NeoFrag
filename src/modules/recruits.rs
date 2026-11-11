//! Module Recrutement : offres, candidatures, avis du staff et décision.
//! Porté de `modules/recruits` et `widgets/recruits`.
//!
//! États d'une candidature (`status`) : 1 en attente, 2 acceptée, 3 refusée.
//! Simplification : l'étude des candidatures est réservée aux administrateurs
//! (l'original la règle par permissions).

use super::{
    account::validate_email,
    admin::{self, admin_guard, Ctxt, CsrfOnly, Notice, Reply},
    teams,
};
use crate::{
    core::{
        bbcode,
        ctx::Ctx,
        files::{Fit, FormData},
        form::{self, checkbox, fits, input, textarea},
        layout::{self, Page},
        mail::{self, Draft},
        panel::{Block, Panel},
        text, throttle, user,
    },
    error::AppError,
};
use axum::{
    extract::{Multipart, Path, Query},
    response::{IntoResponse, Redirect},
    Extension, Form,
};
use chrono::{NaiveDate, NaiveDateTime};
use serde::Deserialize;
use sqlx::Row;
use std::time::Duration;

pub const TITLE: &str = "Recrutement";
pub const ICON: &str = "fas fa-bullhorn";
const STATUSES: [(&str, &str); 4] = [("", ""), ("En attente", "info"), ("Acceptée", "success"), ("Refusée", "danger")];

struct Offer {
    recruit_id: u32,
    title: String,
    introduction: String,
    description: String,
    requirements: String,
    size: i64,
    role: String,
    icon: String,
    date_end: Option<NaiveDate>,
    closed: bool,
    team_id: Option<u32>,
    team_title: Option<String>,
    image: Option<String>,
    image_id: Option<u32>,
    candidacies: i64,
    accepted: i64,
}

impl Offer {
    fn path(&self) -> String {
        format!("recruits/{}/{}", self.recruit_id, text::url_title(&self.title))
    }

    /// Peut-on encore postuler ?
    fn open(&self, today: NaiveDate) -> bool {
        !self.closed && self.date_end.is_none_or(|end| end >= today) && (self.size <= 0 || self.accepted < self.size)
    }

    fn status_badge(&self, today: NaiveDate) -> &'static str {
        if self.open(today) {
            "<span class=\"badge badge-success\">Ouvert</span>"
        } else {
            "<span class=\"badge badge-secondary\">Clôturé</span>"
        }
    }
}

// `requierments` : faute d'orthographe du schéma d'origine, conservée.
const SELECT: &str = "SELECT r.recruit_id, r.title, r.introduction, r.description, r.requierments, CAST(r.size AS SIGNED) AS size, \
        r.role, r.icon, r.date_end, (r.closed = '1') AS is_closed, r.team_id, tl.title AS team_title, r.image_id, f.path AS image, \
        (SELECT COUNT(*) FROM nf_recruits_candidacies c WHERE c.recruit_id = r.recruit_id) AS candidacies, \
        (SELECT COUNT(*) FROM nf_recruits_candidacies c WHERE c.recruit_id = r.recruit_id AND c.status = '2') AS accepted \
     FROM nf_recruits r \
     LEFT JOIN nf_teams_lang tl ON tl.team_id = r.team_id AND tl.lang = ? \
     LEFT JOIN nf_file f ON f.id = r.image_id";

fn from_row(row: &sqlx::mysql::MySqlRow) -> sqlx::Result<Offer> {
    let clean = |name: &str| row.try_get::<String, _>(name).map(|v| text::clean(&v));
    Ok(Offer {
        recruit_id: row.try_get("recruit_id")?,
        title: clean("title")?,
        introduction: clean("introduction")?,
        description: clean("description")?,
        requirements: clean("requierments")?,
        size: row.try_get("size")?,
        role: clean("role")?,
        icon: row.try_get("icon")?,
        date_end: row.try_get("date_end")?,
        closed: row.try_get::<i64, _>("is_closed")? != 0,
        team_id: row.try_get("team_id")?,
        team_title: row.try_get::<Option<String>, _>("team_title")?.map(|t| text::clean(&t)),
        image: row.try_get("image")?,
        image_id: row.try_get("image_id")?,
        candidacies: row.try_get("candidacies")?,
        accepted: row.try_get("accepted")?,
    })
}

async fn all(ctx: &Ctx) -> Result<Vec<Offer>, AppError> {
    let rows = sqlx::query(&format!("{SELECT} ORDER BY r.closed, r.date DESC"))
        .bind(ctx.lang())
        .fetch_all(&ctx.state.db)
        .await?;
    Ok(rows.iter().map(from_row).collect::<sqlx::Result<_>>()?)
}

async fn find(ctx: &Ctx, id: u32) -> Result<Option<Offer>, AppError> {
    let row = sqlx::query(&format!("{SELECT} WHERE r.recruit_id = ?"))
        .bind(ctx.lang())
        .bind(id)
        .fetch_optional(&ctx.state.db)
        .await?;
    Ok(row.as_ref().map(from_row).transpose()?)
}

fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

fn date_fr(date: &NaiveDate) -> String {
    date.format("%d/%m/%Y").to_string()
}

fn offer_summary(ctx: &Ctx, offer: &Offer) -> String {
    let mut meta = vec![offer.status_badge(today()).to_owned()];
    if !offer.role.is_empty() {
        meta.push(format!("{} {}", text::icon(if offer.icon.is_empty() { "fas fa-user" } else { &offer.icon }), offer.role));
    }
    if let Some(team) = &offer.team_title {
        meta.push(format!("{} {team}", text::icon("fas fa-headset")));
    }
    if offer.size > 0 {
        meta.push(format!("{} {} / {} place(s)", text::icon("fas fa-users"), offer.accepted, offer.size));
    }
    if let Some(end) = &offer.date_end {
        meta.push(format!("{} jusqu'au {}", text::icon("far fa-calendar-alt"), date_fr(end)));
    }
    format!(
        "<h5 class=\"card-title\"><a href=\"{}\">{}</a></h5><p class=\"text-muted\">{}</p>{}",
        ctx.url(&offer.path()),
        offer.title,
        meta.join(" <span class=\"mx-1\">·</span> "),
        bbcode::render(&offer.introduction),
    )
}

pub async fn index(Extension(ctx): Ctxt) -> Reply {
    if !ctx.enabled("recruits") {
        return layout::not_found(&ctx).await;
    }
    let mut blocks: Vec<Block> = all(&ctx)
        .await?
        .iter()
        .map(|offer| Block::Panel(Panel::new().body(offer_summary(&ctx, offer))))
        .collect();
    if blocks.is_empty() {
        blocks.push(Block::Panel(
            Panel::new()
                .heading(TITLE, ICON)
                .style("card-info")
                .body("<div class=\"text-center\">Aucune offre de recrutement pour le moment</div>"),
        ));
    }
    layout::render(&ctx, Page::new("recruits", TITLE, ICON).blocks(&blocks)).await
}

/// Candidature déjà déposée par le membre connecté pour cette offre.
async fn my_candidacy(ctx: &Ctx, recruit_id: u32) -> Result<Option<u32>, AppError> {
    let Some(user) = &ctx.user else {
        return Ok(None);
    };
    Ok(sqlx::query_scalar("SELECT candidacy_id FROM nf_recruits_candidacies WHERE recruit_id = ? AND user_id = ? ORDER BY candidacy_id DESC LIMIT 1")
        .bind(recruit_id)
        .bind(user.id)
        .fetch_optional(&ctx.state.db)
        .await?)
}

pub async fn show(Extension(ctx): Ctxt, Path((id, slug)): Path<(u32, String)>) -> Reply {
    if !ctx.enabled("recruits") {
        return layout::not_found(&ctx).await;
    }
    let offer = match find(&ctx, id).await? {
        Some(offer) if text::url_title(&offer.title) == slug => offer,
        _ => return layout::not_found(&ctx).await,
    };
    let image = match &offer.image {
        Some(path) => format!("<img class=\"card-img-top\" src=\"{}\" alt=\"\" />", text::encode(&ctx.url(path))),
        None => String::new(),
    };
    let section = |title: &str, content: &str| {
        if content.is_empty() {
            String::new()
        } else {
            format!("<div class=\"card-body border-top\"><h6>{title}</h6>{}</div>", bbcode::render(content))
        }
    };
    let action = match my_candidacy(&ctx, id).await? {
        Some(candidacy_id) => format!(
            "<a class=\"btn btn-secondary\" href=\"{}\">{} Voir ma candidature</a>",
            ctx.url(&format!("recruits/candidacy/{candidacy_id}/{slug}")),
            text::icon("far fa-eye")
        ),
        None if offer.open(today()) => format!(
            "<a class=\"btn btn-primary\" href=\"{}\">{} Postuler</a>",
            ctx.url(&format!("recruits/postulate/{id}/{slug}")),
            text::icon("fas fa-pencil-alt")
        ),
        None => "<span class=\"text-muted\">Les candidatures sont closes.</span>".to_owned(),
    };
    let body = format!(
        "{image}<div class=\"card-body\">{}</div>{}{}<div class=\"card-footer text-right\">{action}</div>",
        offer_summary(&ctx, &offer),
        section("Description", &offer.description),
        section("Profil recherché", &offer.requirements),
    );
    let page = Page::new("recruits", TITLE, ICON)
        .title(offer.title.clone())
        .crumb(offer.title.clone(), offer.path())
        .blocks(&[Block::Panel(Panel::new().body_raw(body))]);
    layout::render(&ctx, page).await
}

#[derive(Deserialize, Default)]
pub struct ApplyForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    pseudo: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    date_of_birth: String,
    #[serde(default)]
    presentation: String,
    #[serde(default)]
    motivations: String,
    #[serde(default)]
    experiences: String,
}

async fn apply_page(ctx: &Ctx, offer: &Offer, values: &ApplyForm, error: Option<&str>) -> Reply {
    let area = |label: &str, name: &str, value: &str, required: bool| {
        format!(
            "<div class=\"form-group\"><label for=\"f-{name}\"><b>{label}</b></label>\
             <textarea class=\"form-control\" id=\"f-{name}\" name=\"{name}\" rows=\"5\" maxlength=\"5000\"{}>{}</textarea></div>",
            if required { " required" } else { "" },
            text::encode(value)
        )
    };
    let identity = if ctx.user.is_some() {
        String::new()
    } else {
        format!(
            "{}{}",
            input("Votre pseudo", "pseudo", &values.pseudo, 60, true, ""),
            form::email("Adresse email", "email", &values.email, true)
        )
    };
    let body = format!(
        "{}<form method=\"post\" action=\"{}\">{}{identity}{}{}{}{}<button type=\"submit\" class=\"btn btn-primary\">Envoyer ma candidature</button> \
         <a class=\"btn btn-link\" href=\"{}\">Annuler</a></form>",
        form::alert("danger", error),
        ctx.url(&format!("recruits/postulate/{}/x", offer.recruit_id)),
        form::csrf_field(ctx),
        form::date("Date de naissance", "date_of_birth", &values.date_of_birth),
        area("Présentation", "presentation", &values.presentation, true),
        area("Motivations", "motivations", &values.motivations, true),
        area("Expériences", "experiences", &values.experiences, false),
        ctx.url(&offer.path()),
    );
    let page = Page::new("recruits", TITLE, ICON)
        .title("Postuler")
        .crumb(offer.title.clone(), offer.path())
        .crumb("Postuler", format!("recruits/postulate/{}/x", offer.recruit_id))
        .blocks(&[Block::Panel(Panel::new().heading(format!("Postuler — {}", offer.title), "fas fa-pencil-alt").body(body))]);
    layout::render(ctx, page).await
}

/// Offre à laquelle le visiteur peut postuler, sinon la réponse à renvoyer.
async fn applicable(ctx: &Ctx, id: u32) -> Result<Result<Offer, Reply>, AppError> {
    if !ctx.enabled("recruits") {
        return Ok(Err(layout::not_found(ctx).await));
    }
    let Some(offer) = find(ctx, id).await? else {
        return Ok(Err(layout::not_found(ctx).await));
    };
    if !offer.open(today()) || my_candidacy(ctx, id).await?.is_some() {
        return Ok(Err(Ok(Redirect::to(&ctx.url(&offer.path())).into_response())));
    }
    Ok(Ok(offer))
}

pub async fn apply_get(Extension(ctx): Ctxt, Path((id, _slug)): Path<(u32, String)>) -> Reply {
    match applicable(&ctx, id).await? {
        Ok(offer) => apply_page(&ctx, &offer, &ApplyForm::default(), None).await,
        Err(reply) => reply,
    }
}

pub async fn apply_post(Extension(ctx): Ctxt, Path((id, _slug)): Path<(u32, String)>, Form(mut form): Form<ApplyForm>) -> Reply {
    let offer = match applicable(&ctx, id).await? {
        Ok(offer) => offer,
        Err(reply) => return reply,
    };
    for field in [&mut form.pseudo, &mut form.email, &mut form.date_of_birth, &mut form.presentation, &mut form.motivations, &mut form.experiences] {
        *field = field.trim().to_owned();
    }
    if !ctx.csrf_ok(&form.csrf) {
        return apply_page(&ctx, &offer, &form, Some("Votre session a expiré, veuillez réessayer.")).await;
    }
    let db = &ctx.state.db;
    let (user_id, pseudo, email) = match &ctx.user {
        Some(user) => {
            let email: Option<String> = sqlx::query_scalar("SELECT email FROM nf_user WHERE id = ?")
                .bind(user.id)
                .fetch_optional(db)
                .await?
                .flatten();
            (Some(user.id), text::decode(&user.username), email.unwrap_or_default())
        }
        None => (None, form.pseudo.clone(), form.email.clone()),
    };
    let date_of_birth = if form.date_of_birth.is_empty() {
        None
    } else {
        match NaiveDate::parse_from_str(&form.date_of_birth, "%Y-%m-%d") {
            Ok(date) if date < today() => Some(date),
            _ => return apply_page(&ctx, &offer, &form, Some("Date de naissance invalide")).await,
        }
    };
    let too_long = |s: &str| s.chars().count() > 5000;
    let error = if user_id.is_none() && (pseudo.chars().count() < 2 || !fits(&pseudo, 60)) {
        Some("Indiquez votre pseudo (60 caractères maximum).")
    } else if user_id.is_none() && validate_email(&email).is_err() {
        Some("Adresse email invalide.")
    } else if form.presentation.is_empty() || form.motivations.is_empty() {
        Some("La présentation et les motivations sont obligatoires.")
    } else if too_long(&form.presentation) || too_long(&form.motivations) || too_long(&form.experiences) {
        Some("Chaque texte est limité à 5000 caractères.")
    } else {
        None
    };
    if let Some(message) = error {
        return apply_page(&ctx, &offer, &form, Some(message)).await;
    }
    let key = format!("recruits:{}", ctx.ip);
    let hour = Duration::from_secs(3600);
    if throttle::blocked(&ctx.state, &key, 3, hour) {
        return apply_page(&ctx, &offer, &form, Some("Trop de candidatures envoyées, réessayez plus tard.")).await;
    }
    throttle::hit(&ctx.state, &key, hour);

    sqlx::query(
        "INSERT INTO nf_recruits_candidacies (recruit_id, user_id, pseudo, email, date_of_birth, presentation, motivations, experiences, status) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, '1')",
    )
    .bind(id)
    .bind(user_id)
    .bind(text::encode(&pseudo))
    .bind(&email)
    .bind(date_of_birth)
    .bind(text::encode(&form.presentation))
    .bind(text::encode(&form.motivations))
    .bind(text::encode(&form.experiences))
    .execute(db)
    .await?;

    let panel = Panel::new()
        .heading("Candidature envoyée", "fas fa-check")
        .body("<div class=\"text-center\">Votre candidature a bien été enregistrée. Elle sera étudiée par l'équipe.</div>");
    let page = Page::new("recruits", TITLE, ICON)
        .title("Candidature envoyée")
        .crumb(offer.title.clone(), offer.path())
        .blocks(&[Block::Panel(panel)]);
    layout::render(&ctx, page).await
}

struct Candidacy {
    candidacy_id: u32,
    recruit_id: u32,
    date: NaiveDateTime,
    user_id: Option<u32>,
    pseudo: String,
    email: String,
    date_of_birth: Option<NaiveDate>,
    presentation: String,
    motivations: String,
    experiences: String,
    status: usize,
    reply: String,
    offer_title: String,
}

impl Candidacy {
    fn status_badge(&self) -> String {
        let (label, color) = STATUSES[self.status];
        format!("<span class=\"badge badge-{color}\">{label}</span>")
    }
}

const CANDIDACY: &str = "SELECT c.candidacy_id, c.recruit_id, CAST(c.date AS DATETIME) AS date, c.user_id, c.pseudo, c.email, \
        c.date_of_birth, c.presentation, c.motivations, c.experiences, CAST(c.status AS CHAR) AS status, IFNULL(c.reply, '') AS reply, \
        r.title AS offer_title \
     FROM nf_recruits_candidacies c INNER JOIN nf_recruits r ON r.recruit_id = c.recruit_id";

fn candidacy_from(row: &sqlx::mysql::MySqlRow) -> sqlx::Result<Candidacy> {
    let clean = |name: &str| row.try_get::<String, _>(name).map(|v| text::clean(&v));
    Ok(Candidacy {
        candidacy_id: row.try_get("candidacy_id")?,
        recruit_id: row.try_get("recruit_id")?,
        date: row.try_get("date")?,
        user_id: row.try_get("user_id")?,
        pseudo: clean("pseudo")?,
        email: row.try_get("email")?,
        date_of_birth: row.try_get("date_of_birth")?,
        presentation: clean("presentation")?,
        motivations: clean("motivations")?,
        experiences: clean("experiences")?,
        status: row.try_get::<String, _>("status")?.parse::<usize>().unwrap_or(1).clamp(1, 3),
        reply: clean("reply")?,
        offer_title: clean("offer_title")?,
    })
}

async fn find_candidacy(ctx: &Ctx, id: u32) -> Result<Option<Candidacy>, AppError> {
    let row = sqlx::query(&format!("{CANDIDACY} WHERE c.candidacy_id = ?"))
        .bind(id)
        .fetch_optional(&ctx.state.db)
        .await?;
    Ok(row.as_ref().map(candidacy_from).transpose()?)
}

fn candidacy_html(ctx: &Ctx, c: &Candidacy, staff: bool) -> String {
    let who = match c.user_id {
        Some(id) => user::link(ctx.base(), id, &c.pseudo),
        None => c.pseudo.clone(),
    };
    let section = |title: &str, content: &str| {
        if content.is_empty() {
            String::new()
        } else {
            format!("<div class=\"card-body border-top\"><h6>{title}</h6>{}</div>", content.replace('\n', "<br />"))
        }
    };
    format!(
        "<div class=\"card-body\"><div class=\"float-right\">{}</div><h5 class=\"mb-1\">{who}</h5><small class=\"text-muted\">{} {}{}{}</small></div>{}{}{}{}",
        c.status_badge(),
        text::icon("far fa-clock"),
        text::date_time(&c.date),
        match &c.date_of_birth {
            Some(date) => format!(" · né(e) le {}", date_fr(date)),
            None => String::new(),
        },
        if staff && !c.email.is_empty() { format!(" · {}", text::clean(&c.email)) } else { String::new() },
        section("Présentation", &c.presentation),
        section("Motivations", &c.motivations),
        section("Expériences", &c.experiences),
        section("Réponse de l'équipe", &c.reply),
    )
}

/// `recruits/candidacy/{id}/{titre}` : le candidat (membre) suit sa candidature.
pub async fn candidacy(Extension(ctx): Ctxt, Path((id, _slug)): Path<(u32, String)>) -> Reply {
    if !ctx.enabled("recruits") {
        return layout::not_found(&ctx).await;
    }
    let Some(c) = find_candidacy(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let mine = ctx.user.as_ref().is_some_and(|u| Some(u.id) == c.user_id);
    if !mine && !ctx.access.admin() {
        // Même réponse que pour une candidature inexistante.
        return layout::not_found(&ctx).await;
    }
    let offer_path = format!("recruits/{}/{}", c.recruit_id, text::url_title(&c.offer_title));
    let page = Page::new("recruits", TITLE, ICON)
        .title("Ma candidature")
        .crumb(c.offer_title.clone(), offer_path)
        .blocks(&[Block::Panel(
            Panel::new()
                .heading(format!("Candidature — {}", c.offer_title), "far fa-file-alt")
                .body_raw(candidacy_html(&ctx, &c, false)),
        )]);
    layout::render(&ctx, page).await
}

/// Widget : offres ouvertes.
pub async fn widget(ctx: &Ctx) -> Result<Vec<Block>, AppError> {
    let now = today();
    let offers: Vec<Offer> = all(ctx).await?.into_iter().filter(|o| o.open(now)).take(5).collect();
    let panel = Panel::new().heading(TITLE, ICON);
    if offers.is_empty() {
        return Ok(vec![Block::Panel(panel.body("Aucun recrutement en cours"))]);
    }
    let items: String = offers
        .iter()
        .map(|o| {
            format!(
                "<li class=\"list-group-item\"><a href=\"{}\">{}</a><br /><small class=\"text-muted\">{}</small></li>",
                ctx.url(&o.path()),
                o.title,
                o.role
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
    admin_guard!(ctx, "recruits");
    let now = today();
    let mut body = String::new();
    for o in all(&ctx).await? {
        body.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td>{}</tr>",
            o.title,
            o.role,
            o.candidacies,
            o.status_badge(now),
            admin::row_actions(&ctx, &format!("admin/{}", o.path()), &format!("admin/recruits/delete/{}", o.recruit_id), "Supprimer cette offre et ses candidatures ?"),
        ));
    }
    let rows = sqlx::query(&format!("{CANDIDACY} ORDER BY c.status, c.candidacy_id DESC LIMIT 200"))
        .fetch_all(&ctx.state.db)
        .await?;
    let mut candidacies = String::new();
    for row in &rows {
        let c = candidacy_from(row)?;
        candidacies.push_str(&format!(
            "<tr><td><a href=\"{}\">{}</a></td><td>{}</td><td>{}</td><td>{}</td></tr>",
            ctx.url(&format!("admin/recruits/candidacies/{}", c.candidacy_id)),
            c.pseudo,
            c.offer_title,
            text::date_short(&c.date),
            c.status_badge(),
        ));
    }
    let content = format!(
        "{}<div class=\"mt-4\">{}</div>",
        admin::list_card(&ctx, "Offres", Some("admin/recruits/add"), &["Titre", "Rôle", "Candidatures", "État", ""], &body),
        admin::list_card(&ctx, "Candidatures", None, &["Candidat", "Offre", "Date", "État"], &candidacies),
    );
    admin::render(&ctx, "recruits", TITLE, notice.message(), content)
}

#[derive(Default)]
struct Values {
    title: String,
    role: String,
    icon: String,
    size: String,
    team_id: String,
    date_end: String,
    introduction: String,
    description: String,
    requirements: String,
    closed: bool,
}

impl Values {
    fn from_data(data: &FormData) -> Self {
        let get = |name: &str| data.text(name).trim().to_owned();
        Self {
            title: get("title"),
            role: get("role"),
            icon: get("icon"),
            size: get("size"),
            team_id: get("team_id"),
            date_end: get("date_end"),
            introduction: get("introduction"),
            description: get("description"),
            requirements: get("requirements"),
            closed: data.checked("closed"),
        }
    }
}

async fn offer_form(ctx: &Ctx, action: &str, v: &Values, offer: Option<&Offer>, error: Option<&str>) -> Reply {
    let half = |html: String| format!("<div class=\"col-12 col-md-6\">{html}</div>");
    let fields = format!(
        "{}<div class=\"row\">{}{}{}{}{}</div>{}{}{}{}{}",
        input("Titre", "title", &v.title, 100, true, ""),
        half(input("Rôle recherché", "role", &v.role, 60, true, "")),
        half(input("Icône", "icon", &v.icon, 60, false, "Ex. fas fa-crosshairs")),
        half(input("Nombre de places", "size", &v.size, 4, false, "Vide ou 0 : illimité.")),
        half(teams::select(ctx, "Équipe", "team_id", &v.team_id, Some("—")).await?),
        half(form::date("Date limite", "date_end", &v.date_end)),
        admin::image_field(ctx, "Image", "image", offer.and_then(|o| o.image.as_deref()), ""),
        textarea("Introduction", "introduction", &v.introduction, 4, true),
        textarea("Description", "description", &v.description, 8, false),
        textarea("Profil recherché", "requirements", &v.requirements, 6, false),
        checkbox("Offre clôturée", "closed", v.closed),
    );
    let title = if offer.is_some() { "Modifier l'offre" } else { "Ajouter une offre" };
    admin::render(ctx, "recruits", title, None, admin::upload_card(ctx, action, error, fields, "admin/recruits"))
}

async fn save(ctx: &Ctx, multipart: Multipart, offer: Option<Offer>) -> Reply {
    let mut data = admin::read_upload_form(ctx, multipart).await?;
    let v = Values::from_data(&data);
    let action = match &offer {
        Some(o) => format!("admin/{}", o.path()),
        None => "admin/recruits/add".to_owned(),
    };
    let size = if v.size.is_empty() { Some(0) } else { v.size.parse::<u32>().ok().filter(|n| *n <= 1000) };
    let date_end = if v.date_end.is_empty() { Some(None) } else { NaiveDate::parse_from_str(&v.date_end, "%Y-%m-%d").ok().map(Some) };
    let team_id = if v.team_id.is_empty() {
        Some(None)
    } else {
        match v.team_id.parse::<u32>() {
            Ok(id) if teams::find(ctx, id).await?.is_some() => Some(Some(id)),
            _ => None,
        }
    };
    let icon_ok = v.icon.is_empty() || (!text::icon(&v.icon).is_empty() && v.icon.len() <= 60);
    let error = if v.title.is_empty() || text::url_title(&v.title).is_empty() || !fits(&v.title, 100) {
        Some("Le titre est obligatoire (100 caractères maximum).")
    } else if v.role.is_empty() || !fits(&v.role, 60) {
        Some("Le rôle est obligatoire (60 caractères maximum).")
    } else if v.introduction.is_empty() {
        Some("L'introduction est obligatoire.")
    } else if !icon_ok {
        Some("Icône invalide (ex. fas fa-crosshairs).")
    } else if size.is_none() {
        Some("Nombre de places invalide.")
    } else if date_end.is_none() {
        Some("Date limite invalide.")
    } else if team_id.is_none() {
        Some("Équipe invalide.")
    } else {
        None
    };
    let (None, Some(size), Some(date_end), Some(team_id)) = (error, size, date_end, team_id) else {
        return offer_form(ctx, &action, &v, offer.as_ref(), error.or(Some("Formulaire invalide."))).await;
    };
    let Some(author) = &ctx.user else {
        return layout::forbidden(ctx).await;
    };
    let old = offer.as_ref().and_then(|o| o.image_id);
    let image = match admin::image_change(ctx, &mut data, "image", "recruits", Fit::MaxWidth(1600), old).await? {
        Ok(change) => change,
        Err(message) => return offer_form(ctx, &action, &v, offer.as_ref(), Some(message)).await,
    };
    let sql = match &offer {
        Some(_) => {
            "UPDATE nf_recruits SET title = ?, introduction = ?, description = ?, requierments = ?, size = ?, role = ?, icon = ?, \
             date_end = ?, closed = ?, team_id = ?, image_id = ? WHERE recruit_id = ?"
        }
        None => {
            "INSERT INTO nf_recruits (title, introduction, description, requierments, size, role, icon, date_end, closed, team_id, image_id, user_id) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        }
    };
    let query = sqlx::query(sql)
        .bind(text::encode(&v.title))
        .bind(text::encode(&v.introduction))
        .bind(text::encode(&v.description))
        .bind(text::encode(&v.requirements))
        .bind(size)
        .bind(text::encode(&v.role))
        .bind(&v.icon)
        .bind(date_end)
        .bind(if v.closed { "1" } else { "0" })
        .bind(team_id)
        .bind(image.value)
        .bind(match &offer {
            Some(o) => o.recruit_id,
            None => author.id,
        });
    query.execute(&ctx.state.db).await?;
    image.finish(ctx).await?;
    admin::redirect(ctx, "admin/recruits?ok=1")
}

pub async fn admin_add_get(Extension(ctx): Ctxt) -> Reply {
    admin_guard!(ctx, "recruits");
    offer_form(&ctx, "admin/recruits/add", &Values::default(), None, None).await
}

pub async fn admin_add_post(Extension(ctx): Ctxt, multipart: Multipart) -> Reply {
    admin_guard!(ctx, "recruits");
    save(&ctx, multipart, None).await
}

pub async fn admin_edit_get(Extension(ctx): Ctxt, Path((id, _slug)): Path<(u32, String)>) -> Reply {
    admin_guard!(ctx, "recruits");
    let Some(o) = find(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let v = Values {
        title: text::decode(&o.title),
        role: text::decode(&o.role),
        icon: o.icon.clone(),
        size: if o.size > 0 { o.size.to_string() } else { String::new() },
        team_id: o.team_id.map(|t| t.to_string()).unwrap_or_default(),
        date_end: o.date_end.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default(),
        introduction: text::decode(&o.introduction),
        description: text::decode(&o.description),
        requirements: text::decode(&o.requirements),
        closed: o.closed,
    };
    offer_form(&ctx, &format!("admin/{}", o.path()), &v, Some(&o), None).await
}

pub async fn admin_edit_post(Extension(ctx): Ctxt, Path((id, _slug)): Path<(u32, String)>, multipart: Multipart) -> Reply {
    admin_guard!(ctx, "recruits");
    let Some(o) = find(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    save(&ctx, multipart, Some(o)).await
}

pub async fn admin_delete(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<CsrfOnly>) -> Reply {
    admin_guard!(ctx, "recruits");
    admin::check_csrf(&ctx, &form.csrf)?;
    let image = find(&ctx, id).await?.and_then(|o| o.image_id);
    sqlx::query("DELETE FROM nf_recruits WHERE recruit_id = ?")
        .bind(id)
        .execute(&ctx.state.db)
        .await?;
    if let Some(image) = image {
        crate::core::files::delete(&ctx, image).await?;
    }
    admin::redirect(&ctx, "admin/recruits?ok=1")
}

pub async fn admin_candidacy(Extension(ctx): Ctxt, Path(id): Path<u32>, Query(notice): Query<Notice>) -> Reply {
    admin_guard!(ctx, "recruits");
    let Some(c) = find_candidacy(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let votes = sqlx::query(
        "SELECT u.username, (v.vote = '1') AS vote, v.comment FROM nf_recruits_candidacies_votes v \
         INNER JOIN nf_user u ON u.id = v.user_id WHERE v.candidacy_id = ? ORDER BY v.vote_id",
    )
    .bind(id)
    .fetch_all(&ctx.state.db)
    .await?;
    let mut vote_rows = String::new();
    for row in &votes {
        vote_rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
            text::clean(&row.try_get::<String, _>("username")?),
            admin::badge(row.try_get::<i64, _>("vote")? != 0, "Pour", "Contre"),
            text::clean(&row.try_get::<String, _>("comment")?),
        ));
    }
    let content = format!(
        "<div class=\"row\"><div class=\"col-12 col-xl-7\"><div class=\"card\"><div class=\"card-body pb-0\"><h5>{}</h5></div>{}</div></div>\
         <div class=\"col-12 col-xl-5 mt-4 mt-xl-0\"><div class=\"card\"><div class=\"card-body\"><h5>Avis du staff</h5>\
         <form method=\"post\" action=\"{}\">{}<div class=\"form-group\"><textarea class=\"form-control\" name=\"comment\" rows=\"2\" maxlength=\"1000\" placeholder=\"Commentaire (facultatif)\"></textarea></div>\
         <button type=\"submit\" class=\"btn btn-success\" name=\"vote\" value=\"1\">Pour</button> \
         <button type=\"submit\" class=\"btn btn-danger\" name=\"vote\" value=\"0\">Contre</button></form></div>\
         <table class=\"table mb-0\"><tbody>{vote_rows}</tbody></table></div>\
         <div class=\"card mt-4\"><div class=\"card-body\"><h5>Décision</h5><form method=\"post\" action=\"{}\">{}\
         <div class=\"form-group\"><textarea class=\"form-control\" name=\"reply\" rows=\"4\" maxlength=\"5000\" placeholder=\"Réponse au candidat\">{}</textarea>\
         <small class=\"form-text text-muted\">{}</small></div>\
         <button type=\"submit\" class=\"btn btn-success\" name=\"status\" value=\"2\">Accepter</button> \
         <button type=\"submit\" class=\"btn btn-danger\" name=\"status\" value=\"3\">Refuser</button> \
         <button type=\"submit\" class=\"btn btn-light\" name=\"status\" value=\"1\">Remettre en attente</button></form></div></div></div></div>",
        c.offer_title,
        candidacy_html(&ctx, &c, true),
        ctx.url(&format!("admin/recruits/candidacies/{id}/vote")),
        form::csrf_field(&ctx),
        ctx.url(&format!("admin/recruits/candidacies/{id}")),
        form::csrf_field(&ctx),
        c.reply,
        if mail::enabled(&ctx) { "Le candidat est prévenu par email de la décision." } else { "L'envoi d'emails n'est pas configuré : le candidat ne sera pas prévenu par email." },
    );
    admin::render(&ctx, "recruits", "Candidature", notice.message(), content)
}

#[derive(Deserialize)]
pub struct VoteForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    vote: String,
    #[serde(default)]
    comment: String,
}

pub async fn admin_vote(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<VoteForm>) -> Reply {
    admin_guard!(ctx, "recruits");
    admin::check_csrf(&ctx, &form.csrf)?;
    let Some(voter) = &ctx.user else {
        return layout::forbidden(&ctx).await;
    };
    if find_candidacy(&ctx, id).await?.is_none() {
        return layout::not_found(&ctx).await;
    }
    let comment: String = form.comment.trim().chars().take(1000).collect();
    let db = &ctx.state.db;
    // Un avis par membre du staff : le nouveau remplace l'ancien.
    sqlx::query("DELETE FROM nf_recruits_candidacies_votes WHERE candidacy_id = ? AND user_id = ?")
        .bind(id)
        .bind(voter.id)
        .execute(db)
        .await?;
    sqlx::query("INSERT INTO nf_recruits_candidacies_votes (candidacy_id, user_id, vote, comment) VALUES (?, ?, ?, ?)")
        .bind(id)
        .bind(voter.id)
        .bind(if form.vote == "1" { "1" } else { "0" })
        .bind(text::encode(&comment))
        .execute(db)
        .await?;
    admin::redirect(&ctx, &format!("admin/recruits/candidacies/{id}?ok=1"))
}

#[derive(Deserialize)]
pub struct DecisionForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    reply: String,
}

pub async fn admin_decide(Extension(ctx): Ctxt, Path(id): Path<u32>, Form(form): Form<DecisionForm>) -> Reply {
    admin_guard!(ctx, "recruits");
    admin::check_csrf(&ctx, &form.csrf)?;
    let Some(c) = find_candidacy(&ctx, id).await? else {
        return layout::not_found(&ctx).await;
    };
    let status = match form.status.as_str() {
        "2" => "2",
        "3" => "3",
        _ => "1",
    };
    let reply: String = form.reply.trim().chars().take(5000).collect();
    sqlx::query("UPDATE nf_recruits_candidacies SET status = ?, reply = ? WHERE candidacy_id = ?")
        .bind(status)
        .bind(text::encode(&reply))
        .bind(id)
        .execute(&ctx.state.db)
        .await?;

    if status != "1" && validate_email(&c.email).is_ok() {
        let offer = text::decode(&c.offer_title);
        let mut paragraphs = vec![
            format!("Bonjour {},", text::decode(&c.pseudo)),
            format!(
                "Votre candidature pour « {offer} » a été {}.",
                if status == "2" { "acceptée" } else { "refusée" }
            ),
        ];
        if !reply.is_empty() {
            paragraphs.push(reply.clone());
        }
        let subject = format!("Votre candidature — {offer}");
        let draft = Draft { to: &c.email, reply_to: None, subject: &subject, paragraphs: &paragraphs, button: None };
        if let Ok(email) = mail::build(&ctx, draft) {
            tokio::spawn(email.send());
        }
    }
    admin::redirect(&ctx, &format!("admin/recruits/candidacies/{id}?ok=1"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(closed: bool, date_end: Option<NaiveDate>, size: i64, accepted: i64) -> Offer {
        Offer {
            recruit_id: 1,
            title: String::new(),
            introduction: String::new(),
            description: String::new(),
            requirements: String::new(),
            size,
            role: String::new(),
            icon: String::new(),
            date_end,
            closed,
            team_id: None,
            team_title: None,
            image: None,
            image_id: None,
            candidacies: 0,
            accepted,
        }
    }

    #[test]
    fn offers_close_by_flag_date_or_capacity() {
        let day = |d| NaiveDate::from_ymd_opt(2026, 11, d).unwrap();
        assert!(offer(false, None, 0, 5).open(day(11)));
        assert!(!offer(true, None, 0, 0).open(day(11)));
        assert!(offer(false, Some(day(11)), 0, 0).open(day(11)));
        assert!(!offer(false, Some(day(10)), 0, 0).open(day(11)));
        assert!(offer(false, None, 2, 1).open(day(11)));
        assert!(!offer(false, None, 2, 2).open(day(11)));
    }
}
