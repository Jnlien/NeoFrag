# Feuille de route du portage NeoFrag → Rust

Objectif : portage de NeoFrag Alpha 0.2.3 sur **Axum + Askama + SQLx**, en gardant le
**schéma MySQL d'origine** pour pouvoir reprendre un site existant.

**État : tous les modules et widgets de l'original ont un équivalent Rust.** Certains sont
simplifiés ; les écarts sont listés ci-dessous, sans rien cacher. La branche `master`
reste en PHP tant que la liste « Reste à faire » n'est pas traitée et que le tout n'a pas
tourné sur un vrai site.

## Modules

| Module d'origine | Fichier Rust | Écarts avec l'original |
| --- | --- | --- |
| news | `modules/news.rs`, `admin.rs` | — |
| pages | `modules/pages.rs`, `admin.rs` | — |
| members | `modules/members.rs` | — |
| user | `user.rs`, `account.rs`, `recovery.rs`, `messages.rs`, `system.rs` (groupes) | pas de captcha, de message de bienvenue, d'image de couverture ni d'historique des sessions ; pas de nouvelle validation quand un membre change d'email |
| comments | `modules/comments.rs` | pas de réponses imbriquées (inachevées dans l'original) |
| contact | `modules/contact.rs` | — |
| search | `modules/search.rs` | actualités, pages, sujets du forum et membres (pas les autres modules) |
| forum | `modules/forum.rs` | pas de sondages ni de suivi de sujet par email |
| events | `modules/events.rs` | pas d'alerte par message privé |
| teams, games, awards, partners | fichiers du même nom | — |
| gallery | `modules/gallery.rs` | — |
| recruits | `modules/recruits.rs` | étude des candidatures réservée aux administrateurs ; pas d'alerte par message privé |
| talks | `modules/talks.rs` | messages rafraîchis toutes les 10 s ; l'envoi recharge la page |
| access | `modules/system.rs` | réglage par groupe ; pas de règle par membre (les règles existantes sont conservées) ; l'administration se délègue module par module, pas action par action |
| addons | `modules/system.rs` | activation / désactivation des modules et widgets ; pas d'installation par archive, ni thèmes, langues, authentificateurs |
| settings | `modules/settings.rs` | pas de couleurs / fond du thème, captcha, Analytics, authentification HTTP |
| statistics | `modules/system.rs` | tableau mensuel sur 12 mois au lieu des graphiques Highcharts |
| monitoring | `modules/system.rs` | état du serveur et de la base ; pas de mise à jour automatique |
| live_editor | `modules/live_editor.rs` | formulaires au lieu du glisser-déposer ; le style d'une ligne n'est pas modifiable |
| admin | `modules/admin.rs` | administration fonctionnelle mais sobre (pas de tableaux triables, d'éditeur BBCode WysiBB, de sélecteurs d'icônes) |
| tools | — | utilitaires internes de l'original (API de développement), sans objet ici |

Tous les widgets sont portés : `header`, `navigation`, `breadcrumb`, `module`, `copyright`,
`html`, `search`, `user`, `members`, `news`, `forum`, `gallery`, `events`, `teams`,
`partners`, `awards`, `recruits`, `talks`, `about`, `socials`, `slider`.

## Reste à faire

Par ordre d'importance pour une mise en production.

1. **Essai sur une vraie base NeoFrag** : tout a été testé contre le schéma d'origine avec
   des données de test, pas contre un site ayant des années de contenu.
2. **Internationalisation** : les libellés sont en français dans le code. Restent le
   mécanisme `lang()`, les fichiers `langs/*.php` (en, de, es, it, pt), le choix de langue
   par visiteur et la saisie des contenus dans plusieurs langues.
3. **Authentification externe** (`addons/authenticator_*` : Steam, Discord, Twitch, Google,
   GitHub, Battle.net…) via OAuth.
4. **Mises à jour de schéma** pour les bases antérieures à 0.2.3 (l'installation neuve
   se fait avec `neofrag install`).
5. **Couleurs du thème** : `style.css` est la version compilée par défaut ; l'original
   recompile le SCSS avec les couleurs choisies (`azuro_*`). Thème `default` et thèmes tiers.
6. **Confort d'administration** : éditeur BBCode, tableaux triables, glisser-déposer du
   Live Editor, graphiques des statistiques.
7. **Écarts de module** listés dans le tableau ci-dessus (sondages du forum, captcha,
   alertes par message privé, pièces jointes autres que des images…).
8. **Mots de passe** : phpass (MD5 itéré) est conservé car `nf_user.password` fait 34
   caractères. Proposer une migration optionnelle vers Argon2id (au prix de la
   compatibilité avec le site PHP).
9. **Divers** : les anciennes données encodent les accents en entités (`&eacute;`), la
    recherche ne les retrouve pas par leur forme accentuée ; les jetons expirés restent
    dans `nf_user_token` jusqu'à la prochaine demande ; les avatars GIF animés sont figés.
