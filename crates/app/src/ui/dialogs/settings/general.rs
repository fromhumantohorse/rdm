//! "General": language, theme, system integration.

use super::*;

pub(super) fn general_tab(ui: &mut Ui, p: &Palette, s: &mut Settings) {
    section(ui, p, icon::TRANSLATE, tr!("Langue et apparence", "Language and appearance"), |ui| {
        language_menu(ui, &mut s.language);
        ui.add_space(6.0);
        segmented(
            ui,
            &mut s.theme,
            &[
                (Theme::System, icon::DESKTOP, tr!("Système", "System")),
                (Theme::Dark, icon::MOON, tr!("Sombre", "Dark")),
                (Theme::Light, icon::SUN, tr!("Clair", "Light")),
            ],
        );
    });

    section(ui, p, icon::DESKTOP, tr!("Système", "System"), |ui| {
        toggle(ui, &mut s.autostart, tr!("Lancer au démarrage", "Start with the system"), tr!("Démarre réduit dans la zone de notification", "Starts minimised in the notification area"));
        toggle(
            ui,
            &mut s.close_to_tray,
            tr!("Fermer la fenêtre garde RDM actif", "Closing the window keeps RDM running"),
            tr!("Les téléchargements continuent depuis la zone de notification", "Downloads go on from the notification area"),
        );
        toggle(ui, &mut s.notify, tr!("Notifications", "Notifications"), tr!("Téléchargement terminé, verdict VirusTotal", "Download complete, VirusTotal verdict"));
    });
}
