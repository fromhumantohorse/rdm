//! What the user does in the browser to finish an installation, with the folder or file at hand.

use super::*;

pub(super) fn steps(ui: &mut Ui, p: &Palette, manager: &Arc<Manager>, (browser, exe): (Browser, Option<&Path>), done: &Installed, copied: &mut Option<String>) {
    let path_row = |ui: &mut Ui, path: &Path, copied: &mut Option<String>| path_row(ui, p, manager, path, copied);
    let name = browser.name();
    // What "reopen" opens again: the page, or the package the browser was handed.
    let mut reopen = (browser.extensions_page().to_owned(), tr!("Rouvrir la page des extensions", "Reopen the extensions page"));
    let opened = |what: &str| {
        if done.launched { trf!("{name} vient de s'ouvrir sur {what}.", "{name} just opened {what}.", name = name, what = what) } else { trf!("Ouvrez {what} dans {name}.", "Open {what} in {name}.", name = name, what = what) }
    };
    let reopen_package = tr!("Rouvrir le paquet", "Reopen the package");
    let unsigned_xpi = done.xpi.as_deref();
    match (browser.flavour(), unsigned_xpi) {
        (Flavour::Safari, _) => {
            let first = if done.launched {
                tr!("Les réglages de Safari viennent de s'ouvrir sur les extensions : cochez « RDM ».", "Safari's settings just opened on its extensions: tick \"RDM\".")
            } else {
                tr!("Dans Safari : Réglages → Extensions, cochez « RDM ».", "In Safari: Settings → Extensions, tick \"RDM\".")
            };
            step(ui, p, 1, first);
            step(ui, p, 2, tr!("Autorisez-la sur tous les sites : c'est ainsi qu'elle voit les téléchargements.", "Allow it on every website: that is how it sees downloads."));
            step(ui, p, 3, connects_itself());
            note(
                ui,
                p,
                tr!(
                    "RDM n'est pas signé par un compte Apple Developer : Safari ne montre l'extension qu'avec « Autoriser les extensions non signées » (réglages développeur de Safari), à cocher à chaque ouverture de Safari.",
                    "RDM is not signed by an Apple Developer account: Safari only shows the extension with \"Allow unsigned extensions\" (Safari's developer settings), to tick each time Safari opens."
                ),
            );
            ui.add_space(6.0);
            if ghost_button(ui, icon::ARROW_SQUARE_OUT, tr!("Rouvrir les réglages de Safari", "Reopen Safari's settings")).clicked() {
                extension::open_safari_settings();
            }
            return;
        }
        (Flavour::Chromium, _) if done.store => {
            let page = extension::store_page();
            step(ui, p, 1, &opened(&page));
            step(ui, p, 2, tr!("Cliquez sur son bouton d'ajout (« Ajouter à Chrome »…), puis confirmez : le navigateur demande votre accord.", "Click its add button (\"Add to Chrome\"…), then confirm: the browser asks for your go-ahead."));
            step(ui, p, 3, connects_itself());
            reopen = (page, tr!("Rouvrir la page", "Reopen the page"));
        }
        (Flavour::Chromium, _) => {
            let page = opened(browser.extensions_page());
            step(ui, p, 1, &trf!("{page} Activez le « Mode développeur » (interrupteur de la page).", "{page} Turn on \"Developer mode\" (the page's switch).", page = page));
            step(ui, p, 2, tr!("Cliquez sur « Charger l'extension non empaquetée » et choisissez ce dossier :", "Click \"Load unpacked\" and choose this folder:"));
            path_row(ui, &done.folder, copied);
            step(
                ui,
                p,
                3,
                tr!(
                    "L'icône RDM apparaît dans la barre d'outils : ce navigateur passe « Connectée » ici dès son premier échange.",
                    "The RDM icon appears in the toolbar: this browser shows \"Connected\" here as soon as it first talks to RDM."
                ),
            );
            note(
                ui,
                p,
                tr!(
                    "RDM tient ce dossier à jour : l'extension suit les nouvelles versions au prochain démarrage du navigateur. Ne le supprimez pas.",
                    "RDM keeps this folder up to date: the extension follows new versions at the browser's next start. Do not delete it."
                ),
            );
        }
        (Flavour::Firefox, _) if done.signed => {
            let package = extension::base().join("rdm-firefox-signed.xpi");
            let first = if done.launched {
                trf!("{name} demande de confirmer l'ajout de « RDM » : cliquez sur « Ajouter ».", "{name} asks to confirm adding \"RDM\": click \"Add\".", name = name)
            } else {
                trf!("Ouvrez ce fichier avec {name}, puis cliquez sur « Ajouter » :", "Open this file with {name}, then click \"Add\":", name = name)
            };
            step(ui, p, 1, &first);
            if !done.launched {
                path_row(ui, &package, copied);
            }
            step(ui, p, 2, connects_itself());
            note(ui, p, tr!("Version signée par Mozilla : installée pour de bon, mise à jour avec les versions de RDM.", "Signed by Mozilla: installed for good, updated with RDM's versions."));
            reopen = (package.to_string_lossy().into_owned(), reopen_package);
        }
        // Waterfox installs an unsigned package for good (its signature check is off by default).
        (Flavour::Firefox, Some(xpi)) if browser.key() == "waterfox" => {
            let first = if done.launched {
                tr!("Waterfox propose d'ajouter « RDM » : cliquez sur « Ajouter ».", "Waterfox offers to add \"RDM\": click \"Add\".")
            } else {
                tr!(
                    "Ouvrez ce fichier avec Waterfox (ou glissez-le dans sa fenêtre), puis cliquez sur « Ajouter » :",
                    "Open this file with Waterfox (or drop it into its window), then click \"Add\":"
                )
            };
            step(ui, p, 1, first);
            if !done.launched {
                path_row(ui, xpi, copied);
            }
            step(ui, p, 2, connects_itself());
            note(
                ui,
                p,
                tr!(
                    "Installée pour de bon. Après une mise à jour de RDM, réinstallez-la ici pour passer à sa nouvelle version.",
                    "Installed for good. After an RDM update, reinstall it here to get its new version."
                ),
            );
            reopen = (xpi.to_string_lossy().into_owned(), reopen_package);
        }
        (Flavour::Firefox, _) => {
            let page = opened(tr!("about:debugging (« Ce Firefox »)", "about:debugging (\"This Firefox\")"));
            step(ui, p, 1, &trf!("{page} Cliquez sur « Charger un module complémentaire temporaire… ».", "{page} Click \"Load Temporary Add-on…\".", page = page));
            step(ui, p, 2, tr!("Choisissez le fichier manifest.json de ce dossier :", "Choose the manifest.json file of this folder:"));
            path_row(ui, &done.folder, copied);
            step(ui, p, 3, connects_itself());
            note(
                ui,
                p,
                tr!(
                    "Firefox n'installe durablement que les extensions signées par Mozilla : un module temporaire disparaît à la fermeture de Firefox. \
                     Firefox Developer Edition, Nightly, ESR, Waterfox et LibreWolf installent durablement ce fichier :",
                    "Firefox only keeps extensions signed by Mozilla: a temporary add-on goes away when Firefox closes. \
                     Firefox Developer Edition, Nightly, ESR, Waterfox and LibreWolf keep this file installed for good:"
                ),
            );
            if let Some(xpi) = unsigned_xpi {
                path_row(ui, xpi, copied);
            }
        }
    }
    if let Some(exe) = exe {
        ui.add_space(6.0);
        let (target, label) = reopen;
        if ghost_button(ui, icon::ARROW_SQUARE_OUT, label).clicked() {
            let _ = extension::launch(exe, &target);
        }
    }
}

/// The extension reaches RDM through the connector RDM registered with the browser; only when the
/// browser cannot start it (a sandboxed Flatpak or Snap browser) does RDM ask for approval.
fn connects_itself() -> &'static str {
    tr!(
        "L'extension se connecte seule à RDM. Si RDM vous demande d'autoriser cette installation, cliquez sur « Autoriser ».",
        "The extension connects to RDM by itself. If RDM asks you to allow this installation, click \"Allow\"."
    )
}

fn step(ui: &mut Ui, p: &Palette, n: u32, text: &str) {
    ui.horizontal_top(|ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 10.0, p.tint(p.accent, 0.18));
        ui.painter().text(r.center(), eframe::egui::Align2::CENTER_CENTER, n.to_string(), theme::semibold(11.5), p.accent);
        ui.add(Label::new(RichText::new(text).font(theme::regular(13.0)).color(p.text)).wrap());
    });
    ui.add_space(4.0);
}

fn note(ui: &mut Ui, p: &Palette, text: &str) {
    ui.add_space(2.0);
    ui.add(Label::new(RichText::new(text).font(theme::regular(12.0)).color(p.muted)).wrap());
    ui.add_space(4.0);
}

/// A path to paste into the browser's file picker: copy it, or show it in the file manager.
fn path_row(ui: &mut Ui, p: &Palette, manager: &Arc<Manager>, path: &Path, copied: &mut Option<String>) {
    Frame::new().fill(p.surface).corner_radius(10).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let text = path.display().to_string();
            let width = (ui.available_width() - 84.0).max(60.0);
            ui.allocate_ui_with_layout(vec2(width, 24.0), Layout::left_to_right(Align::Center), |ui| {
                ui.add(Label::new(RichText::new(&text).monospace().color(p.text)).truncate()).on_hover_text(&text);
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::icon_button(ui, icon::FOLDER_OPEN, tr!("Afficher dans le dossier", "Show in folder"), None).clicked() {
                    // Selects the item: a folder opens in its parent with it highlighted.
                    reveal_file(path.to_path_buf(), manager);
                }
                if widgets::icon_button(ui, icon::COPY, tr!("Copier le chemin", "Copy the path"), None).clicked() {
                    *copied = Some(text.clone());
                }
            });
        });
    });
    ui.add_space(6.0);
}
