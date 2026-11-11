//! Pagination, portée de `neofrag/libraries/pagination.php` (URL en `…/page/N`).

use super::panel::Panel;

#[derive(Debug, Clone, Copy)]
pub struct Pagination {
    pub page: u32,
    pub per_page: u32,
    pub total: u32,
}

enum Slot {
    Page(u32),
    Gap,
}

impl Pagination {
    pub fn new(page: u32, per_page: i64, total: i64) -> Self {
        Self {
            page: page.max(1),
            per_page: per_page.clamp(1, 1000) as u32,
            total: total.max(0) as u32,
        }
    }

    pub fn pages(&self) -> u32 {
        self.total.div_ceil(self.per_page).max(1)
    }

    pub fn offset(&self) -> u32 {
        (self.page - 1).saturating_mul(self.per_page)
    }

    /// Page demandée au-delà de la dernière : l'original redirige vers la première.
    pub fn out_of_range(&self) -> bool {
        self.page > 1 && self.offset() >= self.total
    }

    fn slots(&self) -> Vec<Slot> {
        let n = self.pages();
        let cur = self.page;
        let page = |r: std::ops::RangeInclusive<u32>| r.map(Slot::Page).collect::<Vec<_>>();
        if n < 7 {
            return page(1..=n);
        }
        let mut slots = Vec::new();
        if cur >= 5 && n - cur >= 5 {
            slots.extend(page(1..=2));
            slots.push(Slot::Gap);
            slots.extend(page(cur - 1..=cur + 1));
            slots.push(Slot::Gap);
            slots.extend(page(n - 1..=n));
        } else if cur > 1 && cur < 5 {
            slots.extend(page(1..=cur + 1));
            slots.push(Slot::Gap);
            slots.extend(page(n - 1..=n));
        } else if cur < n && n - cur < 5 {
            slots.extend(page(1..=2));
            slots.push(Slot::Gap);
            slots.extend(page(cur.saturating_sub(1).max(3)..=n));
        } else {
            slots.extend(page(1..=2));
            slots.push(Slot::Gap);
            slots.extend(page(n - 1..=n));
        }
        slots
    }

    /// Boutons de pagination ; chaîne vide s'il n'y a qu'une page.
    /// `base_url` est l'URL de la première page (déjà encodée pour un attribut HTML).
    pub fn html(&self, base_url: &str) -> String {
        if self.total <= self.per_page {
            return String::new();
        }
        let base_url = base_url.trim_end_matches('/');
        let mut out = String::from("<div class=\"pagination\">");
        for slot in self.slots() {
            match slot {
                Slot::Gap => out.push_str("<span class=\"btn btn-light\">...</span>"),
                Slot::Page(p) => {
                    let class = if p == self.page { "btn-primary" } else { "btn-light" };
                    let suffix = if p > 1 { format!("/page/{p}") } else { String::new() };
                    let href = if base_url.is_empty() && suffix.is_empty() {
                        "/".to_owned()
                    } else {
                        format!("{base_url}{suffix}")
                    };
                    out.push_str(&format!("<a class=\"btn {class}\" href=\"{href}\">{p}</a>"));
                }
            }
        }
        out.push_str("</div>");
        out
    }

    pub fn panel(&self, base_url: &str) -> Option<Panel> {
        let html = self.html(base_url);
        (!html.is_empty()).then(|| Panel::new().style("card-transparent text-right").body_raw(html))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts() {
        let p = Pagination::new(2, 5, 12);
        assert_eq!(p.pages(), 3);
        assert_eq!(p.offset(), 5);
        assert!(!p.out_of_range());
        assert!(Pagination::new(4, 5, 12).out_of_range());
        assert!(!Pagination::new(1, 5, 0).out_of_range());
    }

    #[test]
    fn single_page_has_no_buttons() {
        assert_eq!(Pagination::new(1, 5, 5).html("/news"), "");
    }

    #[test]
    fn links() {
        let html = Pagination::new(2, 5, 12).html("/news");
        assert!(html.contains("<a class=\"btn btn-light\" href=\"/news\">1</a>"));
        assert!(html.contains("<a class=\"btn btn-primary\" href=\"/news/page/2\">2</a>"));
        assert!(html.contains("href=\"/news/page/3\""));
    }

    #[test]
    fn long_ranges_have_gaps_and_never_overflow() {
        for total_pages in 7..40u32 {
            for cur in 1..=total_pages {
                let p = Pagination::new(cur, 1, total_pages as i64);
                let slots = p.slots();
                assert!(slots.iter().any(|s| matches!(s, Slot::Gap)));
                assert!(slots.iter().any(|s| matches!(s, Slot::Page(x) if *x == cur)), "page {cur}/{total_pages}");
                assert!(slots.iter().all(|s| matches!(s, Slot::Gap) || matches!(s, Slot::Page(x) if (1..=total_pages).contains(x))));
            }
        }
    }
}
