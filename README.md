# NeoFrag (Rust)

Réécriture en Rust de [NeoFrag](https://github.com/NeoFrag/NeoFrag), le CMS pour équipes
de jeu et associations esport, dont le développement PHP s'est arrêté à l'Alpha 0.2.3.

- **Un seul binaire** : Axum (HTTP) + Askama (templates compilés) + SQLx (MySQL/MariaDB).
- **Même base de données** : le serveur lit et écrit le schéma de NeoFrag Alpha 0.2.3.
  Il se branche sur la base d'un site existant : comptes, mots de passe, actualités,
  pages, permissions et mise en page (dispositions + widgets) sont repris tels quels.
- **Même rendu** : thème Azuro, mêmes URL (`/news/3/titre`, `/news/tag/…`, `/user/1/pseudo`…).

> **État : socle fonctionnel, portage en cours.** Tous les modules d'origine ne sont pas
> encore portés — la liste précise est dans [ROADMAP.md](ROADMAP.md).

## Ce qui fonctionne

| Domaine | Détail |
| --- | --- |
| Noyau | Réglages (`nf_settings`), sessions (`nf_session`), groupes et permissions (`nf_access`), BBCode, pagination, slugs |
| Thème | Azuro : 8 zones, dispositions par page (`nf_dispositions`), lignes / colonnes / widgets |
| Widgets | `header`, `navigation`, `breadcrumb`, `module`, `user`, `members`, `news`, `search`, `html`, `copyright` |
| Actualités | Liste paginée, détail, catégories, tags |
| Pages | Pages statiques avec permission d'accès par page |
| Membres | Inscription, connexion / déconnexion, espace membre, profil public, liste des membres |
| Compte | Édition du profil (identité, pays, liens), avatar, identifiant, email, mot de passe |
| Fichiers | Envoi d'images (`nf_file`, dossier `upload/`) : avatars, images d'actualités et de catégories |
| Recherche | Actualités et pages |
| Administration | Tableau de bord, actualités, catégories, pages, membres (droits, suppression), réglages de base |

## Installation

Prérequis : Rust 1.80+ et MySQL ou MariaDB.

```bash
cargo build --release
cp neofrag.example.toml neofrag.toml   # puis éditez-le
./target/release/neofrag               # ou : neofrag /chemin/vers/neofrag.toml
```

Le binaire a besoin du dossier `assets/` (ou de celui indiqué par `site.assets_dir`).

### Reprendre un site NeoFrag existant

1. Sauvegardez la base et le dossier `upload/`.
2. Dans `neofrag.toml` : mettez l'URL de la base (`config/db.php`) et recopiez dans
   `security.password_salt` la valeur de `$password['salt']` de `config/password.php`.
3. Pointez `site.upload_dir` sur le dossier `upload/` du site.
4. Lancez le serveur. Les widgets des modules pas encore portés sont simplement ignorés.

### Installation neuve

```bash
mysql -e "CREATE DATABASE neofrag CHARACTER SET utf8mb4"
mysql neofrag < legacy/DATABASE.sql
```

Compte initial : `admin` / `admin123`. **Changez ce mot de passe dès la première
connexion** (Espace membre → Mon compte), avant toute mise en ligne.

### Derrière un reverse proxy

Le serveur écoute en HTTP ; mettez nginx, Caddy ou Apache devant pour le HTTPS, activez
`server.https = true` et transmettez l'adresse du visiteur dans l'en-tête `X-Real-IP`
(utilisée pour l'historique des connexions et la limitation des tentatives).

## Organisation du code

```
src/
  main.rs          routes et démarrage
  config.rs        neofrag.toml
  core/            le « framework » : ctx, session, access, layout, panel, bbcode, php (unserialize)…
  modules/         un fichier par module porté (news, pages, user, members, search, admin)
  widgets/         un fichier par famille de widgets
templates/         gabarits Askama (thème Azuro, vues, administration)
assets/            fichiers statiques repris de l'original (css, js, polices, thèmes)
legacy/            DATABASE.sql d'origine (schéma de référence)
```

Chaque fichier indique en tête de quel fichier PHP il est le portage.

## Différences volontaires avec l'original

- Jeton anti-CSRF sur tous les formulaires ; déconnexion en POST.
- Identifiant de session renouvelé à la connexion ; cookie `HttpOnly` + `SameSite=Lax`.
- Limitation des tentatives de connexion par adresse IP.
- BBCode : URL `javascript:` / `data:` refusées dans `[url]` et `[img]`.
- Images envoyées : type vérifié sur le contenu, image ré-encodée (métadonnées
  supprimées), SVG refusés, taille limitée (`site.max_upload_mb`).
- Mots de passe de 8 caractères minimum ; changer de mot de passe déconnecte les autres
  sessions ; modifier l'identifiant ou l'email demande le mot de passe actuel.
- Tout texte lu en base est ré-encodé avant affichage (une donnée ancienne non encodée
  ne peut pas injecter de HTML).
- Les mots de passe restent au format phpass de NeoFrag (compatibilité du schéma) :
  voir ROADMAP pour le passage à Argon2.

## Développement

```bash
cargo test      # tests unitaires (mots de passe, unserialize PHP, permissions, BBCode, images…)
cargo clippy
RUST_LOG=neofrag=debug cargo run   # signale les widgets non portés rencontrés
```

## Licence

LGPL-3.0-or-later, comme NeoFrag. © 2018 Michaël BILCOT & Jérémy VALENTIN pour l'œuvre
d'origine (thème, feuilles de style, schéma, logique portée) ; voir `COPYING` et
`COPYING.LESSER`.
