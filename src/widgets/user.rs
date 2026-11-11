//! Widgets Espace membre, portés de `widgets/user`.

use crate::{
    core::{
        ctx::Ctx,
        panel::{Block, Panel},
        php::Value,
        text, user,
    },
    modules,
};

/// Barre compacte (zone « Haut » / « Menu ») : liens de connexion ou raccourcis du membre.
pub fn index_mini(ctx: &Ctx, settings: &Value) -> String {
    let align = match settings.get_str("align") {
        Some(align) if !align.is_empty() => text::css_classes(align),
        _ => "justify-content-end".to_owned(),
    };
    let item = |title: &str, path: &str, icon: &str| {
        format!(
            "<li class=\"nav-item\" data-toggle=\"tooltip\" title=\"{title}\"><a class=\"nav-link\" href=\"{}\">{}</a></li>",
            ctx.url(path),
            text::icon(icon)
        )
    };

    let mut out = format!("<ul class=\"nav {align}\">");
    match &ctx.user {
        Some(current) => {
            out.push_str(&format!(
                "<li class=\"nav-item\"><span class=\"nav-link\">Bienvenue <a href=\"{}\">{}</a></span></li>",
                ctx.url("user"),
                current.username
            ));
            if ctx.access.admin() {
                out.push_str(&item("Administration", "admin", "fas fa-tachometer-alt"));
            }
            // Déconnexion en POST (avec jeton) : un simple lien pourrait être déclenché par un site tiers.
            out.push_str(&format!(
                "<li class=\"nav-item\" data-toggle=\"tooltip\" title=\"Déconnexion\">\
                 <form method=\"post\" action=\"{}\" class=\"d-inline\">\
                 <input type=\"hidden\" name=\"csrf\" value=\"{}\" />\
                 <button type=\"submit\" class=\"nav-link btn btn-link\">{}</button></form></li>",
                ctx.url("user/logout"),
                ctx.csrf_token(),
                text::icon("fas fa-times")
            ));
        }
        None => {
            out.push_str(&format!(
                "<li class=\"nav-item\"><a class=\"nav-link\" href=\"{}\">{} Connexion</a></li>",
                ctx.url("user/login"),
                text::icon("fas fa-sign-in-alt")
            ));
            if modules::account::registration_open(ctx) {
                out.push_str(&format!(
                    "<li class=\"nav-item\"><a class=\"nav-link\" href=\"{}\">{} Créer un compte</a></li>",
                    ctx.url("user/register"),
                    text::icon("fas fa-user-plus")
                ));
            }
        }
    }
    out.push_str("</ul>");
    out
}

/// Panel « Espace membre » : formulaire de connexion ou menu du membre.
pub fn index(ctx: &Ctx) -> Vec<Block> {
    let Some(current) = &ctx.user else {
        return vec![Block::Panel(
            Panel::new()
                .heading("Espace membre", "")
                .body(modules::user::login_form(ctx, None)),
        )];
    };

    let entry = |icon: &str, path: &str, label: &str| {
        format!(
            "<li class=\"list-group-item\">{} <a href=\"{}\">{label}</a></li>",
            text::icon(icon),
            ctx.url(path)
        )
    };
    let profile = format!("user/{}/{}", current.id, text::url_title(&current.username));
    let mut body = format!(
        "<div class=\"card-body text-center\"><h6 class=\"mb-3\">Bienvenue <a href=\"{}\">{}</a></h6>{}</div>\
         <ul class=\"list-group list-group-flush\">{}{}{}{}",
        ctx.url("user"),
        current.username,
        user::avatar_html(ctx.base(), current.avatar.as_deref(), current.sex.as_deref(), true, "m-auto"),
        entry("fas fa-user", "user", "Mon espace"),
        entry("far fa-eye", &profile, "Voir mon profil"),
        entry("fas fa-edit", "user/edit", "Modifier mon profil"),
        entry("fas fa-cogs", "user/account", "Gérer mon compte"),
    );
    if ctx.access.admin() {
        body.push_str(&entry("fas fa-tachometer-alt", "admin", "Administration"));
    }
    body.push_str("</ul>");

    let footer = format!(
        "<form method=\"post\" action=\"{}\"><input type=\"hidden\" name=\"csrf\" value=\"{}\" />\
         <button type=\"submit\" class=\"btn btn-link p-0\">{} Se déconnecter</button></form>",
        ctx.url("user/logout"),
        ctx.csrf_token(),
        text::icon("fas fa-times")
    );
    vec![Block::Panel(
        Panel::new().heading("Espace membre", "").body_raw(body).footer(footer, false),
    )]
}
