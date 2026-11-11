//! Module Contact : formulaire envoyé par email à l'adresse du site (`nf_contact`).
//! Porté de `modules/contact`.

use super::account::validate_email;
use crate::{
    core::{
        ctx::Ctx,
        form,
        layout::{self, Page},
        mail::{self, Draft},
        panel::{Block, Panel},
        text, throttle,
    },
    error::AppError,
};
use axum::{extract::Query, response::{IntoResponse, Redirect, Response}, Extension, Form};
use serde::Deserialize;
use std::{sync::Arc, time::Duration};

pub const TITLE: &str = "Contact";
pub const ICON: &str = "far fa-envelope";
const HOUR: Duration = Duration::from_secs(3600);

#[derive(Deserialize, Default)]
pub struct ContactForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
pub struct Sent {
    ok: Option<String>,
}

/// Adresse email du membre connecté, s'il en a une.
async fn member_email(ctx: &Ctx) -> Result<Option<String>, AppError> {
    let Some(user) = &ctx.user else {
        return Ok(None);
    };
    let email: Option<String> = sqlx::query_scalar("SELECT email FROM nf_user WHERE id = ?")
        .bind(user.id)
        .fetch_optional(&ctx.state.db)
        .await?
        .flatten();
    Ok(email.filter(|e| validate_email(e).is_ok()))
}

async fn page(ctx: &Ctx, values: &ContactForm, ask_email: bool, error: Option<&str>, sent: bool) -> Result<Response, AppError> {
    let panel = Panel::new().heading(TITLE, ICON);
    let panel = if !mail::enabled(ctx) {
        panel
            .style("card-info")
            .body("<div class=\"text-center\">Le formulaire de contact n'est pas disponible pour le moment.</div>")
    } else if sent {
        panel.body("<div class=\"alert alert-success mb-0\">Message envoyé</div>")
    } else {
        let fields = format!(
            "{}{}{}<div class=\"form-group\"><label for=\"f-message\"><b>Message</b></label>\
             <textarea class=\"form-control\" id=\"f-message\" name=\"message\" rows=\"8\" maxlength=\"5000\" required>{}</textarea></div>",
            form::alert("danger", error),
            form::input("Sujet", "subject", &values.subject, 100, true, ""),
            if ask_email { form::email("Votre adresse email", "email", &values.email, true) } else { String::new() },
            text::encode(&values.message),
        );
        panel.body(format!(
            "<form method=\"post\" action=\"{}\">{}{fields}<button type=\"submit\" class=\"btn btn-primary\">Envoyer</button></form>",
            ctx.url("contact"),
            form::csrf_field(ctx),
        ))
    };
    layout::render(ctx, Page::new("contact", TITLE, ICON).blocks(&[Block::Panel(panel)])).await
}

pub async fn index(Extension(ctx): Extension<Arc<Ctx>>, Query(sent): Query<Sent>) -> Result<Response, AppError> {
    let ask_email = member_email(&ctx).await?.is_none();
    page(&ctx, &ContactForm::default(), ask_email, None, sent.ok.is_some()).await
}

fn validate(form: &ContactForm) -> Result<(), &'static str> {
    if form.subject.is_empty() || form.message.is_empty() {
        return Err("Le sujet et le message sont obligatoires.");
    }
    if form.subject.chars().count() > 100 {
        return Err("Le sujet est limité à 100 caractères.");
    }
    if form.message.chars().count() > 5000 {
        return Err("Le message est limité à 5000 caractères.");
    }
    Ok(())
}

pub async fn send(Extension(ctx): Extension<Arc<Ctx>>, Form(mut form): Form<ContactForm>) -> Result<Response, AppError> {
    form.subject = form.subject.trim().to_owned();
    form.email = form.email.trim().to_owned();
    form.message = form.message.trim().to_owned();
    let member = member_email(&ctx).await?;
    let ask_email = member.is_none();
    let fail = |message: &'static str| page(&ctx, &form, ask_email, Some(message), false);

    if !mail::enabled(&ctx) {
        return page(&ctx, &form, ask_email, None, false).await;
    }
    if !ctx.csrf_ok(&form.csrf) {
        return fail("Votre session a expiré, veuillez réessayer.").await;
    }
    if let Err(message) = validate(&form) {
        return fail(message).await;
    }
    let sender = match member {
        Some(email) => email,
        None => match validate_email(&form.email) {
            Ok(()) => form.email.clone(),
            Err(message) => return fail(message).await,
        },
    };

    // « anti_flood » de l'original.
    let key = format!("contact:{}", ctx.ip);
    if throttle::blocked(&ctx.state, &key, 3, HOUR) {
        return fail("Trop de messages envoyés, réessayez plus tard.").await;
    }

    let author = match &ctx.user {
        Some(user) => format!("{} ({sender})", text::decode(&user.username)),
        None => sender.clone(),
    };
    let paragraphs = [format!("Message de {author} via le formulaire de contact :"), form.message.clone()];
    let draft = Draft {
        to: ctx.settings.get("nf_contact").trim(),
        reply_to: Some(&sender),
        subject: &form.subject,
        paragraphs: &paragraphs,
        button: None,
    };
    let sent = match mail::build(&ctx, draft) {
        Ok(email) => email.send().await,
        Err(_) => false,
    };
    if !sent {
        return fail("Une erreur s'est produite lors de l'envoi du message").await;
    }
    throttle::hit(&ctx.state, &key, HOUR);
    Ok(Redirect::to(&ctx.url("contact?ok=1")).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation() {
        let ok = ContactForm { subject: "Bonjour".into(), message: "Un message".into(), ..ContactForm::default() };
        assert!(validate(&ok).is_ok());
        assert!(validate(&ContactForm::default()).is_err());
        assert!(validate(&ContactForm { subject: "x".repeat(101), ..ContactForm { message: "m".into(), ..ContactForm::default() } }).is_err());
        assert!(validate(&ContactForm { subject: "s".into(), message: "x".repeat(5001), ..ContactForm::default() }).is_err());
    }
}
