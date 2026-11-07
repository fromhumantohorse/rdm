//! "Security": site logins and the VirusTotal key.

use super::*;

const MAX_LOGINS: usize = 64;

pub(super) fn security_tab(ui: &mut Ui, p: &Palette, secrets: &mut Secrets, key: KeyField<'_>) {
    section(ui, p, icon::KEY, tr!("Identifiants des sites", "Site logins"), |ui| {
        note(
            ui,
            p,
            tr!(
                "Envoyés seulement à ce site (et ses sous-domaines), en HTTPS ; pour un site en HTTP local, écrivez « http://nom ».",
                "Sent only to that site (and its subdomains), over HTTPS; for a local HTTP site, type \"http://name\"."
            ),
        );
        note(
            ui,
            p,
            if cfg!(windows) {
                tr!("Mots de passe chiffrés pour votre seul compte.", "Passwords are encrypted for your account only.")
            } else {
                tr!("Mots de passe gardés dans un fichier que seul votre compte peut lire.", "Passwords are kept in a file only your account can read.")
            },
        );
        ui.add_space(6.0);
        let mut remove = None;
        for (i, login) in secrets.sites.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(text_field(&mut login.host, "example.com", p, 200.0));
                ui.add(text_field(&mut login.user, tr!("identifiant", "user"), p, 150.0));
                ui.add(text_field(&mut login.password, tr!("mot de passe", "password"), p, 150.0).password(true));
                if icon_button(ui, icon::TRASH, tr!("Supprimer", "Delete"), None).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            secrets.sites.remove(i);
        }
        if secrets.sites.len() < MAX_LOGINS && ghost_button(ui, icon::PLUS, tr!("Ajouter un site", "Add a site")).clicked() {
            secrets.sites.push(SiteLogin::default());
        }
    });

    section(ui, p, icon::SHIELD_CHECK, "VirusTotal", |ui| {
        note(
            ui,
            p,
            tr!(
                "Analysez un fichier terminé (650 Mo au maximum) avec plus de 70 antivirus, sans quitter RDM. RDM le cherche d'abord par son empreinte SHA-256 : il n'est envoyé que si VirusTotal ne le connaît pas.",
                "Analyse a completed file (650 MB at most) with more than 70 antivirus engines, without leaving RDM. RDM first looks it up by its SHA-256: it is uploaded only if VirusTotal does not know it."
            ),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let field = ui.add(
                TextEdit::singleline(&mut secrets.virustotal_key)
                    .password(!*key.shown)
                    .hint_text(RichText::new(tr!("Clé API VirusTotal (64 caractères)", "VirusTotal API key (64 characters)")).color(p.faint))
                    .desired_width(ui.available_width() - 90.0)
                    .margin(Margin::symmetric(12, 9))
                    .font(eframe::egui::TextStyle::Monospace),
            );
            if key.focus {
                field.request_focus();
                field.scroll_to_me(Some(Align::Center));
            }
            let (glyph, tip) = if *key.shown { (icon::EYE_SLASH, tr!("Masquer la clé", "Hide the key")) } else { (icon::EYE, tr!("Afficher la clé", "Show the key")) };
            if icon_button(ui, glyph, tip, None).clicked() {
                *key.shown = !*key.shown;
            }
        });
        if secrets.virustotal_key.trim() != secrets.virustotal_key {
            secrets.virustotal_key = secrets.virustotal_key.trim().to_owned();
        }
        ui.horizontal(|ui| {
            if ui.link(RichText::new(format!("{}  {}", icon::KEY, tr!("Obtenir une clé gratuite", "Get a free key"))).font(theme::semibold(13.0))).clicked() {
                open_link(virustotal::KEY_PAGE.to_owned());
            }
            ui.label(RichText::new(tr!("(compte VirusTotal gratuit, onglet « API key »)", "(free VirusTotal account, \"API key\" tab)")).font(theme::regular(12.0)).color(p.faint));
        });
        ui.add_space(6.0);
        widgets::icon_text(
            ui,
            icon::WARNING,
            p.warning,
            tr!(
                "Un fichier envoyé est partagé avec les éditeurs d'antivirus : n'analysez pas vos documents personnels.",
                "An uploaded file is shared with antivirus vendors: do not analyse your personal documents."
            ),
            p.muted,
            12.0,
        );
    });
}
