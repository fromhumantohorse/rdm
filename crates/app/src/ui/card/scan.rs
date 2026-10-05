//! The VirusTotal badge of a finished file.

use super::*;

/// VirusTotal state of a finished file, right-aligned at `right`. Returns its rect and whether it
/// animates.
pub(super) fn scan_badge(ui: &mut Ui, p: &Palette, e: &Entry, right: f32, y: f32, actions: &mut Vec<Action>) -> (Rect, bool) {
    let (glyph, text, color, action, tip) = match &e.scan {
        Scan::None => return (Rect::from_min_max(pos2(right, y), pos2(right, y)), false),
        Scan::Done(r) if r.flagged() == 0 => (
            icon::SHIELD_CHECK,
            format!("{}/{}", r.flagged(), r.engines()),
            p.success,
            Some(Action::ShowReport(e.download.id)),
            tr!("VirusTotal : aucun antivirus ne détecte de menace — voir l'analyse", "VirusTotal: no antivirus detects a threat — see the report").to_owned(),
        ),
        Scan::Done(r) => {
            let n = r.flagged();
            (
                icon::SHIELD_WARNING,
                format!("{n}/{}", r.engines()),
                if r.malicious > 0 { p.danger } else { p.warning },
                Some(Action::ShowReport(e.download.id)),
                trf!("VirusTotal : {n} antivirus signalent ce fichier — voir l'analyse", "VirusTotal: {n} antivirus flag this file — see the report", n = n),
            )
        }
        Scan::Running(stage) => ("", stage_text(*stage), p.accent, None, tr!("Analyse VirusTotal en cours", "VirusTotal analysis in progress").to_owned()),
        Scan::Failed(reason) => (
            icon::SHIELD_SLASH,
            tr!("Réessayer", "Retry").to_owned(),
            p.danger,
            Some(Action::Scan(e.download.id)),
            trf!("Analyse impossible : {reason}. Cliquez pour réessayer.", "Analysis failed: {reason}. Click to retry.", reason = reason),
        ),
    };
    let running = matches!(e.scan, Scan::Running(_));
    let galley = ui.painter().layout_no_wrap(text, theme::semibold(12.0), color);
    let width = galley.size().x + 44.0;
    let rect = Rect::from_min_max(pos2(right - width, y - 14.0), pos2(right, y + 14.0));
    let response = ui.interact(rect, Id::new(("scan", e.download.id)), if action.is_some() { Sense::click() } else { Sense::hover() });
    let hover = ui.ctx().animate_bool_with_time(response.id, response.hovered(), 0.12);
    let painter = ui.painter();
    painter.rect(rect, 14, p.tint(color, 0.13 + 0.10 * hover), Stroke::new(theme::HAIRLINE, p.tint(color, 0.35)), StrokeKind::Inside);
    let icon_center = pos2(rect.left() + 17.0, y);
    if running {
        widgets::spinner(painter, icon_center, 6.5, color, ui.input(|i| i.time) as f32);
    } else {
        painter.text(icon_center, Align2::CENTER_CENTER, glyph, theme::regular(16.0), color);
    }
    painter.galley(pos2(rect.left() + 31.0, y - galley.size().y / 2.0), galley, color);
    if let Some(action) = action
        && response.on_hover_text(tip).clicked()
    {
        actions.push(action);
    }
    (rect, running)
}

fn stage_text(stage: Stage) -> String {
    match stage {
        Stage::Queued => tr!("VirusTotal · en attente", "VirusTotal · waiting").into(),
        Stage::Hashing => tr!("VirusTotal · empreinte…", "VirusTotal · hashing…").into(),
        Stage::LookingUp => tr!("VirusTotal · recherche…", "VirusTotal · looking up…").into(),
        Stage::Uploading(f) => {
            let pct = (f * 100.0).floor() as u32;
            trf!("VirusTotal · envoi {pct} %", "VirusTotal · uploading {pct} %", pct = pct)
        }
        Stage::Analyzing => tr!("VirusTotal · analyse…", "VirusTotal · analyzing…").into(),
    }
}
