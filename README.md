# NeoFrag (Rust)

Réécriture en Rust de [NeoFrag](https://github.com/NeoFrag/NeoFrag), le CMS pour équipes
de jeu et associations esport, dont le développement PHP s'est arrêté à l'Alpha 0.2.3.

- **Un seul binaire** : Axum (HTTP) + Askama (templates compilés) + SQLx (MySQL/MariaDB).
- **Même base de données** : le serveur lit et écrit le schéma de NeoFrag Alpha 0.2.3.
  Il se branche sur la base d'un site existant : comptes, mots de passe, actualités,
  pages, permissions et mise en page (dispositions + widgets) sont repris tels quels.
- **Même rendu** : thème Azuro, mêmes URL (`/news/3/titre`, `/news/tag/…`, `/user/1/pseudo`…).

> **État : tous les modules d'origine sont portés, certains en version simplifiée.** Pas
> encore éprouvé sur un site réel — écarts et reste à faire dans [ROADMAP.md](ROADMAP.md).

## Ce qui fonctionne

| Domaine | Détail |
| --- | --- |
| Noyau | Réglages (`nf_settings`), sessions (`nf_session`), groupes et permissions (`nf_access`), BBCode, pagination, slugs |
| Thème | Azuro : 8 zones, dispositions par page (`nf_dispositions`), lignes / colonnes / widgets |
| Widgets | Tous ceux de l'original : `header`, `navigation`, `breadcrumb`, `module`, `copyright`, `html`, `search`, `user`, `members`, `news`, `forum`, `gallery`, `events`, `teams`, `partners`, `awards`, `recruits`, `talks`, `about`, `socials`, `slider` |
| Actualités | Liste paginée, détail, catégories, tags |
| Pages | Pages statiques avec permission d'accès par page |
| Membres | Inscription (avec validation par email si le site l'exige), connexion / déconnexion, mot de passe oublié, espace membre, profil public, liste des membres, groupes |
| Compte | Édition du profil (identité, pays, liens), avatar, identifiant, email, mot de passe |
| Messagerie | Messages privés entre membres (conversations à plusieurs, non-lus) |
| Forum | Catégories, forums et sous-forums, sujets, messages, annonces, verrouillage, déplacement, permissions par catégorie |
| Événements | Agenda, types, matchs (adversaire, manches, scores), participants |
| Équipes, Jeux | Équipes, rôles et joueurs ; jeux, cartes et modes |
| Palmarès, Partenaires | Palmarès par équipe et par jeu ; partenaires avec compteur de visites |
| Galerie | Albums, catégories, envoi de plusieurs images, commentaires |
| Recrutement | Offres, candidatures (visiteurs et membres), avis du staff, décision avec réponse par email |
| Discussions | Tchat par discussion, permissions lire / écrire / supprimer |
| Fichiers | Envoi d'images (`nf_file`, dossier `upload/`) |
| Commentaires | Sur les actualités, les événements et la galerie |
| Contact | Formulaire envoyé par email à l'adresse du site |
| Emails | SMTP (`[email]` dans `neofrag.toml`) ; sans configuration, les fonctions concernées sont désactivées |
| Recherche | Actualités, pages, sujets du forum, membres |
| Administration | Tous les modules ci-dessus, membres et groupes, permissions, Live Editor (zones et widgets), activation des modules et widgets, paramètres, mode maintenance, statistiques, supervision |

Ce qui manque encore par rapport à l'original (langues, connexion Steam / Discord…,
écarts module par module) est détaillé dans
[ROADMAP.md](ROADMAP.md).

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
4. Lancez le serveur. Un widget inconnu (ajouté par une extension tierce) est simplement ignoré.

### Installation neuve

1. Créez une base MySQL / MariaDB vide (`CREATE DATABASE neofrag CHARACTER SET utf8mb4;`).
2. Générez les deux secrets et recopiez-les dans `neofrag.toml`, avec l'URL de la base :
   `neofrag secrets`
3. Lancez l'installation : `neofrag install` crée le schéma, puis demande le nom du site
   et le compte administrateur. Elle refuse de toucher à une base qui contient déjà des
   tables NeoFrag.
4. Lancez le serveur : `neofrag`.

Pour une installation automatisée, les réponses peuvent venir de l'environnement :
`NEOFRAG_SITE_NAME`, `NEOFRAG_ADMIN_USER`, `NEOFRAG_ADMIN_EMAIL`, `NEOFRAG_ADMIN_PASSWORD`.

### Emails

Le mot de passe oublié, la validation d'inscription et le formulaire de contact envoient
des emails. Renseignez la section `[email]` (serveur SMTP) **et** `server.public_url`
(l'adresse publique du site, utilisée pour les liens). L'expéditeur est l'adresse de
contact du site (réglage `nf_contact`). Sans cette configuration, ces fonctions sont
masquées, et les inscriptions sont fermées si le site exige une validation par email.

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
  modules/         un fichier par module porté (news, forum, events, gallery, live_editor, system…)
  widgets/         un fichier par famille de widgets
templates/         gabarits Askama (thème Azuro, vues, administration)
assets/            fichiers statiques repris de l'original (css, js, polices, thèmes)
legacy/            DATABASE.sql d'origine (schéma de référence)
```

Chaque fichier indique en tête de quel fichier PHP il est le portage.

## Différences volontaires avec l'original

- Jeton anti-CSRF sur tous les formulaires ; déconnexion en POST.
- Identifiant de session renouvelé à la connexion ; cookie `HttpOnly` + `SameSite=Lax`.
- Limitation des tentatives de connexion par adresse IP, et des envois (messages, candidatures, tchat).
- Les liens saisis (réseaux sociaux, menus) n'acceptent que des adresses web : jamais `javascript:`.
- Permissions : les actions de modération sont refusées par défaut (liste blanche).
- BBCode : URL `javascript:` / `data:` refusées dans `[url]` et `[img]`.
- Images envoyées : type vérifié sur le contenu, image ré-encodée (métadonnées
  supprimées), SVG refusés, taille limitée (`site.max_upload_mb`).
- Liens envoyés par email : ils expirent (2 h pour un mot de passe, 7 jours pour une
  validation), ne servent qu'une fois, et la base n'en garde qu'une empreinte. La
  demande de réinitialisation ne révèle pas si une adresse est inscrite.
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
