//! A card's right-click menu.

use super::*;

pub(super) fn context_menu(ui: &mut Ui, p: &Palette, queues: &[Queue], e: &Entry, actions: &mut Vec<Action>) {
    ui.set_min_width(270.0);
    let d = &e.download;
    let item = |ui: &mut Ui, glyph: &str, label: &str, color: Color32| {
        ui.add(eframe::egui::Button::new(RichText::new(format!("{glyph}   {label}")).font(theme::regular(13.5)).color(color)).frame(false))
            .clicked()
    };
    let done = *d.status() == Status::Completed;
    if done {
        if item(ui, icon::ARROW_SQUARE_OUT, tr!("Ouvrir", "Open"), p.text) {
            actions.push(Action::Open(d.target.clone()));
        }
        let scan = match &e.scan {
            Scan::Done(_) => Some((tr!("Voir l'analyse VirusTotal", "See the VirusTotal report"), Action::ShowReport(d.id))),
            Scan::None | Scan::Failed(_) if e.scannable() => Some((tr!("Analyser avec VirusTotal", "Scan with VirusTotal"), Action::Scan(d.id))),
            _ => None,
        };
        if let Some((label, action)) = scan
            && item(ui, icon::SHIELD_CHECK, label, p.text)
        {
            actions.push(action);
        }
        match &e.sha256 {
            Some(hash) => {
                let short = hash.get(..10).unwrap_or(hash);
                if item(ui, icon::FINGERPRINT, &trf!("Copier le SHA-256 ({short}…)", "Copy the SHA-256 ({short}…)", short = short), p.text) {
                    actions.push(Action::Copy(hash.clone(), "SHA-256"));
                }
            }
            None => {
                if item(ui, icon::FINGERPRINT, tr!("Calculer le SHA-256", "Compute the SHA-256"), p.text) {
                    actions.push(Action::Hash(d.id));
                }
            }
        }
    }
    if !d.is_recording() {
        if item(ui, icon::SEAL_CHECK, tr!("Vérifier une empreinte…", "Verify a checksum…"), p.text) {
            actions.push(Action::Edit(d.id, Field::Checksum));
        }
        if !done {
            if item(ui, icon::LINK_SIMPLE, tr!("Changer le lien…", "Change the link…"), p.text) {
                actions.push(Action::Edit(d.id, Field::Url));
            }
            if item(ui, icon::GAUGE, tr!("Limiter la vitesse…", "Limit the speed…"), p.text) {
                actions.push(Action::Edit(d.id, Field::SpeedLimit));
            }
            if !queues.is_empty() {
                ui.menu_button(RichText::new(format!("{}   {}", icon::QUEUE, tr!("Déplacer vers", "Move to"))).font(theme::regular(13.5)).color(p.text), |ui| {
                    for queue in std::iter::once(0).chain(queues.iter().map(|q| q.id)) {
                        let mark = if d.queue == queue { "● " } else { "   " };
                        if ui.button(format!("{mark}{}", queue_name(queues, queue))).clicked() {
                            actions.push(Action::MoveToQueue(d.id, queue));
                            ui.close();
                        }
                    }
                });
            }
            let (label, color) = if d.insecure {
                (tr!("Ne plus accepter le certificat non valide", "Stop accepting the invalid certificate"), p.text)
            } else {
                (tr!("Accepter un certificat non valide (ce téléchargement)", "Accept an invalid certificate (this download)"), p.warning)
            };
            if item(ui, icon::SHIELD_WARNING, label, color) {
                actions.push(Action::SetInsecure(d.id, !d.insecure));
            }
        }
    }
    ui.separator();
    if item(ui, icon::FOLDER_OPEN, tr!("Afficher dans le dossier", "Show in folder"), p.text) {
        actions.push(Action::Reveal(d.target.clone()));
    }
    let link = if d.is_recording() { tr!("Copier le lien de la page", "Copy the page link") } else { tr!("Copier le lien", "Copy the link") };
    if item(ui, icon::LINK, link, p.text) {
        actions.push(Action::Copy(d.url.to_string(), tr!("Lien", "Link")));
    }
    if item(ui, icon::COPY, tr!("Copier le chemin", "Copy the path"), p.text) {
        actions.push(Action::Copy(d.target.display().to_string(), tr!("Chemin", "Path")));
    }
    ui.separator();
    if item(ui, icon::X, tr!("Retirer de la liste", "Remove from the list"), p.text) {
        actions.push(Action::Remove(d.id, false));
    }
    if item(ui, icon::TRASH, tr!("Supprimer le fichier", "Delete the file"), p.danger) {
        actions.push(Action::Remove(d.id, true));
    }
}
