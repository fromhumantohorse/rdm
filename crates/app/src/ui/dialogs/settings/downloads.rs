//! "Downloads": folders, sorting, existing files, speed limit, queues.

use super::*;

/// Named queues the settings let the user create (besides the main one).
const MAX_QUEUES: usize = 12;

pub(super) fn downloads_tab(ui: &mut Ui, p: &Palette, s: &mut Settings) {
    section(ui, p, icon::FOLDER, tr!("Dossier de téléchargement", "Download folder"), |ui| {
        note(ui, p, tr!("Là où vont vos fichiers.", "Where your files go."));
        ui.add_space(4.0);
        if let Some(Some(dir)) = folder_row(ui, p, icon::HARD_DRIVES, tr!("Dossier principal", "Main folder"), &s.download_dir, false) {
            s.download_dir = dir;
        }
    });

    section(ui, p, icon::FOLDERS, tr!("Classement par type", "Sorting by type"), |ui| {
        toggle(
            ui,
            &mut s.categorize,
            tr!("Ranger chaque type dans son sous-dossier", "Sort each type into its own sub-folder"),
            tr!("Vidéos, Musique, Compressés… dans le dossier principal", "Videos, Music, Archives… inside the main folder"),
        );
        ui.add_space(4.0);
        for category in Category::ALL {
            let shown = s.category_dir(category);
            match folder_row(ui, p, category_icon(category), crate::i18n::category(category), &shown, s.category_dirs.contains_key(&category)) {
                Some(Some(dir)) => {
                    s.category_dirs.insert(category, dir);
                }
                Some(None) => {
                    s.category_dirs.remove(&category);
                }
                None => {}
            }
        }
    });

    section(ui, p, icon::COPY, tr!("Si le fichier existe déjà", "If the file already exists"), |ui| {
        segmented(
            ui,
            &mut s.existing,
            &[
                (ExistingFile::Ask, icon::CHAT_CIRCLE, tr!("Demander", "Ask")),
                (ExistingFile::Rename, icon::COPY, tr!("Renommer", "Rename")),
                (ExistingFile::Overwrite, icon::ARROWS_CLOCKWISE, tr!("Remplacer", "Overwrite")),
                (ExistingFile::Skip, icon::PROHIBIT, tr!("Ignorer", "Skip")),
            ],
        );
        note(
            ui,
            p,
            match s.existing {
                ExistingFile::Ask => tr!(
                    "RDM passe au premier plan et vous laisse choisir : garder les deux fichiers, ou remplacer l'ancien.",
                    "RDM comes to the front and lets you choose: keep both files, or replace the old one."
                ),
                ExistingFile::Rename => tr!("Le nouveau fichier devient « nom (1).ext ».", "The new file becomes \"name (1).ext\"."),
                ExistingFile::Overwrite => tr!("L'ancien fichier est remplacé par le nouveau.", "The old file is replaced by the new one."),
                ExistingFile::Skip => tr!("Le lien n'est pas téléchargé de nouveau.", "The link is not downloaded again."),
            },
        );
        note(
            ui,
            p,
            tr!(
                "S'applique à tous les téléchargements : du navigateur, des liens collés ou copiés.",
                "Applies to every download: from the browser, from pasted or copied links."
            ),
        );
    });

    section(ui, p, icon::GAUGE, tr!("Limite de vitesse", "Speed limit"), |ui| {
        ui.horizontal(|ui| {
            let mut mib = f64::from(s.speed_limit_kib) / 1024.0;
            ui.add(DragValue::new(&mut mib).range(0.0..=10_000.0).speed(0.1).max_decimals(1).suffix(tr!(" Mo/s", " MB/s")));
            let text = if s.speed_limit_kib == 0 { tr!("limite de vitesse globale : aucune (0)", "global speed limit: none (0)") } else { tr!("limite de vitesse globale", "global speed limit") };
            ui.label(RichText::new(text).color(p.muted));
            s.speed_limit_kib = (mib * 1024.0).round() as u32;
        });
        note(
            ui,
            p,
            tr!(
                "0 = aucune limite. Pratique pour garder du débit pour naviguer ou pour les appels vidéo.",
                "0 = no limit. Useful to keep bandwidth for browsing or video calls."
            ),
        );
    });

    section(ui, p, icon::QUEUE, tr!("Files d'attente", "Queues"), |ui| {
        note(
            ui,
            p,
            tr!(
                "Chaque file a son propre nombre de téléchargements simultanés. Clic droit sur un téléchargement › « Déplacer vers ».",
                "Each queue has its own number of simultaneous downloads. Right-click a download › \"Move to\"."
            ),
        );
        ui.add_space(6.0);
        let mut remove = None;
        for (i, queue) in s.queues.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(text_field(&mut queue.name, tr!("Nom de la file", "Queue name"), p, 220.0).char_limit(40));
                ui.add(Slider::new(&mut queue.max_parallel, 1..=MAX_PARALLEL).text(tr!("simultanés", "at once")));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icon_button(ui, icon::TRASH, tr!("Supprimer la file (ses téléchargements vont dans la file principale)", "Delete the queue (its downloads go to the main queue)"), None).clicked() {
                        remove = Some(i);
                    }
                });
            });
        }
        if let Some(i) = remove {
            s.queues.remove(i);
        }
        if s.queues.len() < MAX_QUEUES && ghost_button(ui, icon::PLUS, tr!("Nouvelle file", "New queue")).clicked() {
            let id = (1..).find(|id| !s.queues.iter().any(|q| q.id == *id)).unwrap_or(1);
            let n = s.queues.len() + 1;
            s.queues.push(Queue { id, name: trf!("File {n}", "Queue {n}", n = n), max_parallel: 1 });
        }
    });
}

/// `icon  label  …path…  [Browse…] [↺]` — returns the new folder, or `Some(None)` to reset.
fn folder_row(ui: &mut Ui, p: &Palette, glyph: &str, label: &str, shown: &std::path::Path, custom: bool) -> Option<Option<std::path::PathBuf>> {
    let mut change = None;
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(vec2(170.0, 30.0), Layout::left_to_right(Align::Center), |ui| {
            widgets::icon_text(ui, glyph, p.muted, label, p.text, 13.5);
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if custom && icon_button(ui, icon::ARROW_COUNTER_CLOCKWISE, tr!("Revenir au dossier par défaut", "Back to the default folder"), None).clicked() {
                change = Some(None);
            }
            if ghost_button(ui, icon::FOLDER_OPEN, tr!("Parcourir", "Browse")).clicked()
                && let Some(dir) = rfd::FileDialog::new().set_directory(shown).pick_folder()
            {
                change = Some(Some(dir));
            }
            let text = RichText::new(path_tail(shown)).monospace().color(if custom { p.accent } else { p.muted });
            ui.add(Label::new(text).truncate()).on_hover_text(shown.display().to_string());
        });
    });
    change
}

/// The distinguishing end of a path: `…\Downloads\Videos`.
fn path_tail(path: &std::path::Path) -> String {
    let parts: Vec<_> = path.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let sep = std::path::MAIN_SEPARATOR_STR;
    if parts.len() <= 3 { path.display().to_string() } else { format!("…{sep}{}", parts[parts.len() - 2..].join(sep)) }
}
