//! Envoi d'emails par SMTP, porté de `neofrag/libraries/email.php`.
//!
//! Comme l'original, l'expéditeur est l'adresse de contact du site (`nf_contact`) et
//! son nom celui du site (`nf_name`). Les messages sont construits à partir de texte
//! brut (paragraphes + bouton facultatif) : la version HTML et la version texte en
//! découlent, et rien de ce que saisit un visiteur n'est interprété comme du HTML.

use super::{ctx::Ctx, text};
use crate::config::Config;
use lettre::{
    message::{header::ContentType, Mailbox, MultiPart, SinglePart},
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use std::{sync::Arc, time::Duration};

pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    /// Adresse publique du site (`https://exemple.fr`), pour les liens.
    public_url: String,
}

impl Mailer {
    /// `None` si l'envoi d'emails n'est pas configuré.
    pub fn from_config(config: &Config) -> anyhow::Result<Option<Self>> {
        let email = &config.email;
        if email.smtp_host.is_empty() {
            return Ok(None);
        }
        if config.server.public_url.is_empty() {
            tracing::warn!("email.smtp_host est défini mais pas server.public_url : envoi d'emails désactivé");
            return Ok(None);
        }

        let builder = match email.smtp_security.as_str() {
            "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(&email.smtp_host)?,
            "none" => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&email.smtp_host),
            _ => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&email.smtp_host)?,
        };
        let mut builder = builder.port(email.smtp_port).timeout(Some(Duration::from_secs(15)));
        if !email.smtp_username.is_empty() {
            builder = builder.credentials(Credentials::new(
                email.smtp_username.clone(),
                email.smtp_password.clone(),
            ));
        }
        Ok(Some(Self {
            transport: builder.build(),
            public_url: config.server.public_url.clone(),
        }))
    }
}

/// Un email prêt à partir. Indépendant de la requête : peut être envoyé en tâche de fond.
pub struct Email {
    mailer: Arc<Mailer>,
    message: Message,
}

impl Email {
    /// Envoie le message. En cas d'échec, l'erreur est journalisée et `false` renvoyé.
    pub async fn send(self) -> bool {
        match self.mailer.transport.send(self.message).await {
            Ok(_) => true,
            Err(e) => {
                tracing::error!(error = %e, "envoi d'email impossible");
                false
            }
        }
    }
}

pub struct Draft<'a> {
    pub to: &'a str,
    /// Adresse de réponse (formulaire de contact).
    pub reply_to: Option<&'a str>,
    pub subject: &'a str,
    /// Paragraphes en texte brut.
    pub paragraphs: &'a [String],
    /// Bouton : (libellé, chemin interne du site).
    pub button: Option<(&'a str, &'a str)>,
}

/// Les fonctions qui envoient des emails sont-elles disponibles ?
pub fn enabled(ctx: &Ctx) -> bool {
    ctx.state.mailer.is_some()
}

/// Retire ce qui permettrait de glisser un second en-tête dans un sujet.
fn header_safe(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).take(200).collect()
}

fn render(site_name: &str, site_url: &str, paragraphs: &[String], button: Option<(&str, &str)>) -> (String, String) {
    let mut html = String::from(
        "<!DOCTYPE html><html><body style=\"margin:0;padding:24px;background:#f4f4f4;font-family:Arial,Helvetica,sans-serif;color:#333;\">\
         <div style=\"max-width:560px;margin:0 auto;background:#ffffff;border-radius:4px;overflow:hidden;\">",
    );
    html.push_str(&format!(
        "<div style=\"background:#1f2428;color:#ffffff;padding:20px 24px;font-size:20px;font-weight:bold;\">{}</div><div style=\"padding:24px;font-size:15px;line-height:1.5;\">",
        text::encode(site_name)
    ));
    let mut plain = String::new();
    for paragraph in paragraphs {
        html.push_str(&format!(
            "<p style=\"margin:0 0 16px;\">{}</p>",
            text::encode(paragraph).replace('\n', "<br />")
        ));
        plain.push_str(paragraph);
        plain.push_str("\n\n");
    }
    if let Some((label, url)) = button {
        html.push_str(&format!(
            "<p style=\"margin:24px 0;text-align:center;\"><a href=\"{0}\" style=\"background:#00d7b3;color:#ffffff;text-decoration:none;padding:12px 20px;border-radius:4px;display:inline-block;\">{1}</a></p>\
             <p style=\"margin:0;font-size:12px;color:#888;\">Si le bouton ne fonctionne pas, copiez cette adresse dans votre navigateur :<br />{0}</p>",
            text::encode(url),
            text::encode(label)
        ));
        plain.push_str(&format!("{label} : {url}\n\n"));
    }
    html.push_str(&format!(
        "</div><div style=\"padding:12px 24px;background:#fafafa;font-size:12px;color:#888;\">{} — <a href=\"{1}\" style=\"color:#888;\">{1}</a></div></div></body></html>",
        text::encode(site_name),
        text::encode(site_url)
    ));
    plain.push_str(&format!("-- \n{site_name} — {site_url}\n"));
    (html, plain)
}

/// Prépare un email. Erreur (message affichable) si l'envoi n'est pas configuré ou si
/// une adresse est invalide.
pub fn build(ctx: &Ctx, draft: Draft) -> Result<Email, &'static str> {
    const UNAVAILABLE: &str = "L'envoi d'emails n'est pas disponible sur ce site.";
    let Some(mailer) = ctx.state.mailer.clone() else {
        return Err(UNAVAILABLE);
    };

    let site_name = text::decode(ctx.settings.get("nf_name"));
    let from_address = ctx.settings.get("nf_contact").trim().to_ascii_lowercase();
    let from = Mailbox::new(
        Some(header_safe(&site_name)),
        from_address.parse().map_err(|_| {
            tracing::error!("le réglage nf_contact n'est pas une adresse email valide");
            UNAVAILABLE
        })?,
    );
    let to: Mailbox = draft.to.parse().map_err(|_| "Adresse email invalide.")?;

    let site_url = format!("{}{}", mailer.public_url, ctx.url(""));
    let button_url = draft.button.map(|(_, path)| absolute_url(ctx, &mailer, path));
    let button = draft
        .button
        .zip(button_url.as_deref())
        .map(|((label, _), url)| (label, url));
    let (html, plain) = render(&site_name, &site_url, draft.paragraphs, button);

    let mut builder = Message::builder().from(from).to(to).subject(header_safe(draft.subject));
    if let Some(reply_to) = draft.reply_to {
        builder = builder.reply_to(reply_to.parse().map_err(|_| "Adresse email invalide.")?);
    }
    let message = builder
        .multipart(
            MultiPart::alternative()
                .singlepart(SinglePart::builder().header(ContentType::TEXT_PLAIN).body(plain))
                .singlepart(SinglePart::builder().header(ContentType::TEXT_HTML).body(html)),
        )
        .map_err(|_| UNAVAILABLE)?;
    Ok(Email { mailer, message })
}

fn absolute_url(ctx: &Ctx, mailer: &Mailer, path: &str) -> String {
    format!("{}{}", mailer.public_url, ctx.url(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_text_is_never_html() {
        let paragraphs = vec!["Bonjour <b>x</b>,\nligne 2".to_owned()];
        let (html, plain) = render("Ma <Team>", "https://exemple.fr/", &paragraphs, Some(("Valider", "https://exemple.fr/user/validation/abc")));
        assert!(html.contains("Bonjour &lt;b&gt;x&lt;/b&gt;,<br />ligne 2"));
        assert!(html.contains("Ma &lt;Team&gt;"));
        assert!(html.contains("href=\"https://exemple.fr/user/validation/abc\""));
        assert!(plain.contains("Bonjour <b>x</b>,\nligne 2"));
        assert!(plain.contains("Valider : https://exemple.fr/user/validation/abc"));
    }

    #[test]
    fn subjects_cannot_inject_headers() {
        assert_eq!(header_safe("Sujet\r\nBcc: victime@exemple.fr"), "SujetBcc: victime@exemple.fr");
        assert_eq!(header_safe(&"x".repeat(500)).len(), 200);
    }
}
