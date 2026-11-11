//! Le « panel » (carte Bootstrap) de NeoFrag, porté de `neofrag/libraries/panel.php`.

use super::text;

#[derive(Debug, Default, Clone)]
pub struct Panel {
    title: String,
    icon: String,
    body: String,
    /// Faux quand le corps apporte déjà ses propres balises (`card-body`, `list-group`…).
    body_tags: bool,
    footer: String,
    footer_class: String,
    style: String,
}

impl Panel {
    pub fn new() -> Self {
        Self {
            body_tags: true,
            ..Self::default()
        }
    }

    /// `title` est du HTML déjà encodé (convention NeoFrag).
    pub fn heading(mut self, title: impl Into<String>, icon: &str) -> Self {
        self.title = title.into();
        self.icon = icon.to_owned();
        self
    }

    pub fn body(mut self, html: impl Into<String>) -> Self {
        self.body = html.into();
        self.body_tags = true;
        self
    }

    pub fn body_raw(mut self, html: impl Into<String>) -> Self {
        self.body = html.into();
        self.body_tags = false;
        self
    }

    pub fn footer(mut self, html: impl Into<String>, align_right: bool) -> Self {
        self.footer = html.into();
        self.footer_class = if align_right { " text-right" } else { "" }.to_owned();
        self
    }

    pub fn style(mut self, classes: &str) -> Self {
        self.style = text::css_classes(classes);
        self
    }

    /// Titre imposé par le widget (`nf_widgets.title`) : remplace le libellé, garde l'icône.
    pub fn set_title(&mut self, title: &str) {
        self.title = title.to_owned();
    }

    pub fn set_style(&mut self, classes: &str) {
        self.style = text::css_classes(classes);
    }

    pub fn render(&self) -> String {
        let mut out = String::from("<div class=\"card");
        if !self.style.is_empty() {
            out.push(' ');
            out.push_str(&self.style);
        }
        out.push_str("\">");
        if !self.title.is_empty() {
            out.push_str("<h6 class=\"card-header\">");
            let icon = text::icon(&self.icon);
            if !icon.is_empty() {
                out.push_str(&icon);
                out.push(' ');
            }
            out.push_str(&self.title);
            out.push_str("</h6>");
        }
        if !self.body.is_empty() {
            if self.body_tags {
                out.push_str("<div class=\"card-body\">");
                out.push_str(&self.body);
                out.push_str("</div>");
            } else {
                out.push_str(&self.body);
            }
        }
        if !self.footer.is_empty() {
            out.push_str("<div class=\"card-footer");
            out.push_str(&self.footer_class);
            out.push_str("\">");
            out.push_str(&self.footer);
            out.push_str("</div>");
        }
        out.push_str("</div>");
        out
    }
}

/// Sortie d'un widget ou d'un module : une suite de panels et de fragments HTML.
#[derive(Debug, Clone)]
pub enum Block {
    Panel(Panel),
    Html(String),
}

pub fn render_blocks(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(|block| match block {
            Block::Panel(panel) => panel.render(),
            Block::Html(html) => html.clone(),
        })
        .collect()
}
