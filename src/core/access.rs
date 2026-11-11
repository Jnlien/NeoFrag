//! Groupes et permissions, portés de `neofrag/core/groups.php` et `access.php`.
//!
//! Une permission est identifiée par `(module, action, id)`. Chaque permission liste
//! des entités (groupes ou utilisateurs) autorisées ou refusées. Les administrateurs
//! passent toujours ; pour les autres, on applique la règle du premier groupe de
//! l'utilisateur dans l'ordre d'affichage des groupes.

use sqlx::{MySqlPool, Row};
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct Permission {
    entity: String,
    is_user: bool,
    authorized: bool,
}

#[derive(Debug, Clone)]
enum Rule {
    /// Ligne `nf_access` sans détail d'entité : ouvert à tous.
    Everyone,
    Permissions(Vec<Permission>),
}

#[derive(Debug, Clone, Default)]
pub struct Access {
    rules: HashMap<(String, String, u32), Rule>,
    /// Identifiants de groupes dans l'ordre de priorité (`admins`, groupes perso, `members`, `visitors`).
    group_order: Vec<String>,
    /// Groupes de l'utilisateur courant.
    user_groups: Vec<String>,
    user_id: Option<u32>,
    is_admin: bool,
}

impl Access {
    pub async fn load(db: &MySqlPool, user: Option<(u32, bool)>) -> sqlx::Result<Self> {
        let mut rules: HashMap<(String, String, u32), Rule> = HashMap::new();

        // Même ordre que l'original : permissions nominatives d'abord, autorisations avant refus.
        let rows = sqlx::query(
            "SELECT a.module, a.action, a.id, IFNULL(ad.entity, '') AS entity, \
                    IFNULL(ad.type = 'user', 0) AS is_user, IFNULL(ad.authorized = '1', 0) AS authorized \
             FROM nf_access a LEFT JOIN nf_access_details ad ON a.access_id = ad.access_id \
             ORDER BY (ad.type <> 'user'), ad.authorized DESC",
        )
        .fetch_all(db)
        .await?;

        for row in rows {
            let key = (
                row.try_get::<String, _>("module")?,
                row.try_get::<String, _>("action")?,
                row.try_get::<u32, _>("id")?,
            );
            let entity: String = row.try_get("entity")?;
            if entity.is_empty() {
                rules.insert(key, Rule::Everyone);
                continue;
            }
            let permission = Permission {
                entity,
                is_user: row.try_get::<i64, _>("is_user")? != 0,
                authorized: row.try_get::<i64, _>("authorized")? != 0,
            };
            match rules.entry(key).or_insert_with(|| Rule::Permissions(Vec::new())) {
                Rule::Permissions(list) => list.push(permission),
                Rule::Everyone => {}
            }
        }

        let mut group_order = vec!["admins".to_owned()];
        let custom = sqlx::query("SELECT group_id FROM nf_groups WHERE auto = '0' ORDER BY `order`")
            .fetch_all(db)
            .await?;
        for row in custom {
            group_order.push(row.try_get::<u32, _>("group_id")?.to_string());
        }
        group_order.push("members".to_owned());
        group_order.push("visitors".to_owned());

        let mut user_groups = Vec::new();
        let (user_id, is_admin) = match user {
            Some((id, admin)) => {
                user_groups.push(if admin { "admins" } else { "members" }.to_owned());
                let rows = sqlx::query("SELECT group_id FROM nf_users_groups WHERE user_id = ?")
                    .bind(id)
                    .fetch_all(db)
                    .await?;
                for row in rows {
                    user_groups.push(row.try_get::<u32, _>("group_id")?.to_string());
                }
                (Some(id), admin)
            }
            None => {
                user_groups.push("visitors".to_owned());
                (None, false)
            }
        };

        Ok(Self {
            rules,
            group_order,
            user_groups,
            user_id,
            is_admin,
        })
    }

    /// L'utilisateur courant a-t-il la permission `(module, action, id)` ?
    pub fn can(&self, module: &str, action: &str, id: u32) -> bool {
        if self.is_admin {
            return true;
        }
        let permissions = match self.rules.get(&(module.to_owned(), action.to_owned(), id)) {
            Some(Rule::Everyone) => return true,
            Some(Rule::Permissions(list)) => list,
            None => return false,
        };

        let denied = permissions.iter().filter(|p| !p.is_user && !p.authorized).count();
        let allowed = permissions.iter().filter(|p| !p.is_user && p.authorized).count();
        // Sans règle pour son groupe : autorisé si la permission est surtout une liste de refus
        // (liste noire), refusé si c'est surtout une liste d'autorisations (liste blanche).
        let default = if denied + allowed > 0 { denied > allowed } else { true };

        let mut by_group: HashMap<&str, bool> = HashMap::new();
        for permission in permissions {
            if permission.is_user {
                if self.user_id.map(|id| id.to_string()).as_deref() == Some(permission.entity.as_str()) {
                    return permission.authorized;
                }
            } else if self.user_groups.contains(&permission.entity) {
                by_group.insert(&permission.entity, permission.authorized);
            }
        }

        self.group_order
            .iter()
            .filter(|group| group.as_str() != "admins" && self.user_groups.contains(group))
            .map(|group| by_group.get(group.as_str()).copied().unwrap_or(default))
            .next()
            .unwrap_or(default)
    }

    /// La permission a-t-elle des règles détaillées (et pas seulement « tout le monde ») ?
    pub fn detailed(&self, module: &str, action: &str, id: u32) -> bool {
        matches!(self.rules.get(&(module.to_owned(), action.to_owned(), id)), Some(Rule::Permissions(_)))
    }

    /// Accès à l'administration. (Pour l'instant : administrateurs uniquement, voir ROADMAP.)
    pub fn admin(&self) -> bool {
        self.is_admin
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access(user_groups: &[&str], user_id: Option<u32>, perms: &[(&str, bool, bool)]) -> Access {
        let mut rules = HashMap::new();
        rules.insert(
            ("pages".to_owned(), "access_page".to_owned(), 1),
            Rule::Permissions(
                perms
                    .iter()
                    .map(|(entity, is_user, authorized)| Permission {
                        entity: (*entity).to_owned(),
                        is_user: *is_user,
                        authorized: *authorized,
                    })
                    .collect(),
            ),
        );
        Access {
            rules,
            group_order: ["admins", "3", "members", "visitors"].map(str::to_owned).to_vec(),
            user_groups: user_groups.iter().map(|g| (*g).to_owned()).collect(),
            user_id,
            is_admin: user_groups.contains(&"admins"),
        }
    }

    #[test]
    fn unknown_permission_is_denied_except_for_admins() {
        let visitor = access(&["visitors"], None, &[]);
        assert!(!visitor.can("news", "add_news", 0));
        let admin = access(&["admins"], Some(1), &[]);
        assert!(admin.can("news", "add_news", 0));
    }

    #[test]
    fn group_rules() {
        // Visiteurs refusés explicitement, les autres passent par défaut.
        let perms = [("visitors", false, false)];
        assert!(!access(&["visitors"], None, &perms).can("pages", "access_page", 1));
        assert!(access(&["members"], Some(2), &perms).can("pages", "access_page", 1));

        // Autant d'autorisations que de refus : le défaut devient « refusé ».
        let perms = [("3", false, true), ("visitors", false, false)];
        assert!(access(&["members", "3"], Some(2), &perms).can("pages", "access_page", 1));
        assert!(!access(&["members"], Some(2), &perms).can("pages", "access_page", 1));
        assert!(!access(&["visitors"], None, &perms).can("pages", "access_page", 1));
    }

    #[test]
    fn user_rule_wins_over_groups() {
        let perms = [("2", true, false), ("members", false, true)];
        assert!(!access(&["members"], Some(2), &perms).can("pages", "access_page", 1));
        assert!(access(&["members"], Some(5), &perms).can("pages", "access_page", 1));
    }
}
