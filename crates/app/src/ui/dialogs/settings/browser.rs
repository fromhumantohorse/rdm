//! "Browser": extension, captured types, clipboard.

use super::*;

pub(super) fn browser_tab(ui: &mut Ui, p: &Palette, s: &mut Settings) -> Option<FormAction> {
    let mut action = None;
    section(ui, p, icon::PUZZLE_PIECE, tr!("Extension du navigateur", "Browser extension"), |ui| {
        note(
            ui,
            p,
            tr!(
                "L'extension envoie à RDM les téléchargements de votre navigateur.",
                "The extension hands your browser's downloads over to RDM."
            ),
        );
        ui.add_space(8.0);
        if accent_button(ui, icon::PUZZLE_PIECE, tr!("Installer l'extension…", "Install the extension…")).clicked() {
            action = Some(FormAction::Window(Action::OpenBrowsers));
        }
        ui.add_space(8.0);
        toggle(
            ui,
            &mut s.confirm_browser,
            tr!("Confirmer les téléchargements du navigateur", "Confirm downloads from the browser"),
            tr!("RDM passe au premier plan et attend votre accord", "RDM comes to the front and waits for your go-ahead"),
        );
        if !s.confirm_browser {
            ui.add_space(4.0);
            unconfirmed_warning(ui, p);
        }
        toggle(
            ui,
            &mut s.mini_button,
            tr!("Mini bouton de téléchargement", "Mini download button"),
            tr!("Une petite icône sans texte sur les vidéos et les modèles 3D", "A small icon without text over videos and 3D models"),
        );
        toggle(
            ui,
            &mut s.offer_extension,
            tr!("Proposer l'extension au lancement d'un navigateur", "Offer the extension when a browser starts"),
            tr!("Quand un navigateur sans l'extension démarre, RDM propose de l'installer", "When a browser without the extension starts, RDM offers to install it"),
        );
    });

    section(ui, p, icon::FILE_ARROW_DOWN, tr!("Types de fichiers capturés", "Captured file types"), |ui| {
        ui.add(TextEdit::multiline(&mut s.captured).desired_rows(3).desired_width(f32::INFINITY).font(eframe::egui::TextStyle::Monospace));
        note(
            ui,
            p,
            tr!(
                "Extensions séparées par des espaces. Les archives découpées (.r00, .001…) sont toujours capturées.",
                "Extensions separated by spaces. Split archives (.r00, .001…) are always captured."
            ),
        );
        ui.add_space(4.0);
        if ghost_button(ui, icon::ARROW_COUNTER_CLOCKWISE, tr!("Liste par défaut", "Default list")).clicked() {
            s.captured = domain::default_captured();
        }
    });

    section(ui, p, icon::CLIPBOARD_TEXT, tr!("Liens copiés dans le presse-papiers", "Links copied to the clipboard"), |ui| {
        segmented(
            ui,
            &mut s.clipboard,
            &[
                (ClipboardMode::Off, icon::PROHIBIT, tr!("Ignorer", "Ignore")),
                (ClipboardMode::Ask, icon::CHAT_CIRCLE, tr!("Proposer", "Offer")),
                (ClipboardMode::Auto, icon::LIGHTNING, tr!("Télécharger", "Download")),
            ],
        );
        note(
            ui,
            p,
            tr!(
                "Seuls les liens vers un format capturé sont pris en compte ; rien n'est envoyé nulle part.",
                "Only links to a captured format count; nothing is sent anywhere."
            ),
        );
    });
    action
}
