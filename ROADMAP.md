# Feuille de route du portage NeoFrag → Rust

Objectif : portage **complet et fidèle** de NeoFrag Alpha 0.2.3 (≈ 50 000 lignes de PHP hors
bibliothèques tierces), sur **Axum + Askama + SQLx**, en gardant le **schéma MySQL
d'origine** pour pouvoir reprendre un site existant.

Les tailles indiquées sont celles du code PHP d'origine : elles donnent l'ordre de grandeur
du travail restant.

## Fait

| Élément | Origine PHP | Fichier Rust |
| --- | --- | --- |
| Configuration | `config/*.php` | `src/config.rs` |
| Réglages | `core/config.php` | `src/core/settings.rs` |
| Sessions | `core/session.php` | `src/core/session.rs` |
| Groupes et permissions | `core/groups.php`, `core/access.php` | `src/core/access.rs` |
| Mots de passe phpass | `libraries/password.php` | `src/core/password.rs` |
| Sortie, zones, dispositions | `core/output.php`, `displayables/*` | `src/core/layout.rs` |
| Panel | `libraries/panel.php` | `src/core/panel.rs` |
| Pagination | `libraries/pagination.php` | `src/core/pagination.rs` |
| BBCode | `libraries/bbcode.php` | `src/core/bbcode.rs` |
| Helpers de texte | `helpers/string.php`, `assets.php` | `src/core/text.rs` |
| `unserialize` / `serialize` PHP | (natif PHP) | `src/core/php.rs` |
| Thème Azuro (rendu) | `themes/azuro` | `templates/azuro.html` |
| Module Actualités (public) | `modules/news` | `src/modules/news.rs` |
| Module Pages (public) | `modules/pages` | `src/modules/pages.rs` |
| Module Membres | `modules/members` | `src/modules/members.rs` |
| Module Utilisateur (connexion, profil public) | `modules/user` (partiel) | `src/modules/user.rs` |
| Inscription, édition du profil, avatar, identifiants, mot de passe | `modules/user/forms/*` | `src/modules/account.rs` |
| Envoi de fichiers (images) | `models/file.php` | `src/core/files.rs` |
| Formulaires | `libraries/form*.php` (partiel) | `src/core/form.rs` |
| Pays | `helpers/countries.php` | `src/core/countries.rs` |
| Limitation des tentatives | (absent de l'original) | `src/core/throttle.rs` |
| Emails (SMTP) | `libraries/email.php`, `config/email.php` | `src/core/mail.rs` |
| Jetons envoyés par email | `modules/user/models/token.php` | `src/core/token.rs` |
| Validation d'inscription, mot de passe oublié | `modules/user` (actions `lost_password`) | `src/modules/recovery.rs` |
| Module Contact | `modules/contact` | `src/modules/contact.rs` |
| Module Commentaires (public) | `modules/comments` | `src/modules/comments.rs` |
| Recherche (actualités, pages) | `modules/search` (partiel) | `src/modules/search.rs` |
| Administration (actualités et catégories avec images, pages, membres, réglages de base) | `modules/admin`, `news`, `pages`, `settings` (partiel) | `src/modules/admin.rs` |
| Widgets header, navigation, breadcrumb, module, user, members, news, search, html, copyright | `widgets/*` | `src/widgets/` |

## Reste à faire

### 1. Socle manquant (à faire avant les gros modules)

- **Module Utilisateur, suite** : captcha, message de bienvenue, image de couverture,
  messagerie privée, historique des sessions, administration des groupes, nouvelle
  validation quand un membre change d'adresse email.
- **Envoi de fichiers, suite** : seules les images sont gérées ; les autres types
  (pièces jointes) viendront avec les modules qui en ont besoin. Les avatars GIF animés
  sont figés (ré-encodage).
- **Commentaires, suite** : administration (`admin/comments`), réponses à un commentaire
  (jamais terminées dans l'original), commentaires sur les autres modules à mesure
  qu'ils sont portés.
- **Jetons** : la table `nf_user_token` n'a pas de date ; les jetons expirés non utilisés
  y restent (inutilisables) jusqu'à la prochaine validation ou réinitialisation du compte.
- **Administration des permissions** (`modules/access`, 610 lignes) et accès à
  l'administration par permission de module (aujourd'hui : administrateurs uniquement).
- **Internationalisation** : les libellés sont en français dans le code, comme dans
  l'original ; il reste à porter le mécanisme `lang()` et les fichiers `langs/*.php`
  (en, de, es, it, pt), le choix de langue par visiteur et les contenus multilingues.
- **Thème d'administration** (`themes/admin`) : tableaux triables, éditeur BBCode
  (WysiBB), sélecteurs d'icônes et de couleurs. L'administration actuelle est fonctionnelle
  mais minimale.
- **Mode maintenance**, captcha, statistiques de visites, détection fine des robots.
- **Couleurs du thème** : `style.css` est la version compilée par défaut ; l'original
  recompile le SCSS avec les couleurs choisies (`azuro_*`). À porter avec la crate `grass`.
- **Liens automatiques** dans le BBCode (`strtolink`).

### 2. Modules restants

| Module | Lignes PHP | Remarques |
| --- | ---: | --- |
| forum | 2 500 | catégories, sujets, messages, sondages, suivi, modération, permissions par forum |
| recruits | 2 100 | offres, candidatures, votes |
| events | 2 100 | événements, matchs, adversaires, manches, participants |
| gallery | 1 600 | albums, catégories, images (dépend de l'envoi de fichiers) |
| teams | 1 250 | équipes, rôles, joueurs ; fournit des groupes aux permissions |
| awards | 1 150 | palmarès |
| monitoring | 1 050 | état du serveur — à repenser pour un binaire Rust |
| settings | 1 000 | tous les réglages (aujourd'hui : nom, description, accueil, actualités par page) |
| games | 1 000 | jeux, cartes, modes |
| addons | 800 | activation des modules / widgets / thèmes / langues (`nf_addon`) |
| live_editor | 700 | édition visuelle des dispositions (réécriture de `nf_dispositions`) |
| talks | 600 | discussions instantanées |
| partners | 600 | partenaires |
| statistics | 260 | graphiques de fréquentation |
| tools | 170 | utilitaires |

### 3. Widgets restants

`partners`, `talks`, `slider`, `forum`, `events`, `gallery`, `recruits`, `teams`, `awards`,
`socials`, `about` — chacun se porte avec son module. En attendant, ils sont ignorés au
rendu (visibles avec `RUST_LOG=neofrag=debug`).

### 4. Autres

- **Authentification externe** (`addons/authenticator_*` : Steam, Discord, Twitch, Google,
  GitHub, Battle.net…) via OAuth.
- **Thème `default`** et installation de thèmes tiers.
- **Assistant d'installation** (`install/`) : création de la base et du compte admin.
- **Mises à jour de schéma** (`neofrag/install/*.php`) pour les bases antérieures à 0.2.3.
- **Mots de passe** : phpass (MD5 itéré) est conservé car `nf_user.password` fait 34
  caractères. Proposer une migration optionnelle : élargir la colonne et re-hacher en
  Argon2id à la connexion (au prix de la compatibilité avec le site PHP).
- **Recherche** : les anciennes données encodent les accents en entités (`&eacute;`) ;
  la recherche ne les retrouve pas encore par leur forme accentuée.
- Les fichiers JS / SCSS des modules non portés (`assets/modules/*`) contiennent encore
  des appels PHP ; ils sont à nettoyer au portage de chaque module.

## Ordre conseillé

1. ~~Envoi de fichiers~~ (fait) → 2. ~~Utilisateur : inscription, profil, mot de passe~~ (fait) →
3. ~~Emails (validation d'inscription, mot de passe oublié, contact)~~ (fait) →
4. ~~Commentaires~~ (fait) → 5. Permissions + addons → 6. Équipes / Jeux → 7. Événements → 8. Forum →
9. Galerie, Partenaires, Palmarès, Recrutement → 10. Live editor →
11. Thème d'administration → 12. Internationalisation → 13. OAuth, installation.

## Décisions prises

- **Périmètre** : portage complet 1:1.
- **Architecture** : Axum + templates rendus côté serveur (Askama), un seul binaire.
- **Base** : MySQL/MariaDB, schéma NeoFrag conservé.
- Les dispositions sont cherchées par module/action (`news/_news/*`), comme l'original.
