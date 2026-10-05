//! Why a download stopped, in the interface's current language.

use domain::Failure;

use crate::{manager::RouteError, tr, trf};

/// A short sentence: no URL (it may carry tokens), the reason only.
pub fn text(failure: &Failure) -> String {
    match failure {
        Failure::RedirectRefused => tr!("redirection refusée (boucle ou vers le réseau local)", "redirect refused (loop, or into the local network)").into(),
        Failure::InvalidCertificate => tr!(
            "certificat du site non valide (clic droit › Accepter un certificat non valide, si vous faites confiance au site)",
            "the site's certificate is not valid (right-click › Accept an invalid certificate, if you trust the site)"
        )
        .into(),
        Failure::Status(status) => status_text(*status),
        Failure::TimedOut => tr!("délai dépassé, connexion trop lente", "timed out, connection too slow").into(),
        Failure::CannotConnect => tr!("connexion impossible au serveur", "cannot connect to the server").into(),
        Failure::ConnectionInterrupted => tr!("connexion interrompue", "connection interrupted").into(),
        Failure::Disk(e) => trf!("erreur disque : {e}", "disk error: {e}", e = e),
        Failure::InconsistentRange => tr!("le serveur a renvoyé une plage incohérente", "the server sent an inconsistent range").into(),
        Failure::NoData => tr!("le serveur n'a renvoyé aucune donnée (lien expiré ou protégé)", "the server sent no data (link expired or protected)").into(),
        Failure::CutBeforeTheEnd => tr!("connexion coupée avant la fin", "connection cut before the end").into(),
        Failure::LocalNetwork => tr!("bloqué : un contenu Internet visait votre réseau local", "blocked: Internet content pointing into your local network").into(),
        Failure::Stream(m) => trf!("flux vidéo : {m}", "video stream: {m}", m = m),
        Failure::Merge(e) => trf!("fusion audio/vidéo impossible : {e}", "cannot merge audio and video: {e}", e = e),
        Failure::InvalidProxy => RouteError::InvalidProxy.to_string(),
        Failure::RecordingInterrupted => tr!("enregistrement interrompu (RDM fermé)", "recording interrupted (RDM closed)").into(),
        Failure::RecordingAbandoned => tr!(
            "enregistrement interrompu (onglet fermé ou lecture arrêtée)",
            "recording interrupted (tab closed or playback stopped)"
        )
        .into(),
        Failure::RecordingTooLarge => tr!("enregistrement anormalement volumineux", "abnormally large recording").into(),
        Failure::RecordingCancelled => tr!("enregistrement annulé", "recording cancelled").into(),
        Failure::RecordingEmpty => tr!("aucune donnée vidéo et audio reçue", "no video or audio data received").into(),
        Failure::Text(text) => text.clone(),
    }
}

fn status_text(status: u16) -> String {
    match status {
        401 => tr!("identifiants requis (Paramètres › Identifiants des sites)", "login required (Settings › Site logins)").into(),
        403 => tr!("accès refusé par le serveur (lien expiré ou protégé)", "access denied by the server (link expired or protected)").into(),
        404 => tr!("fichier introuvable (404)", "file not found (404)").into(),
        410 => tr!("lien expiré (410)", "link expired (410)").into(),
        429 => tr!("le serveur limite les connexions (429)", "the server limits connections (429)").into(),
        s @ 500..=599 => trf!("erreur du serveur ({s})", "server error ({s})", s = s),
        s => trf!("le serveur a répondu {s}", "the server answered {s}", s = s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{self, Language};

    #[test]
    fn failures_read_in_the_language_of_the_moment() {
        let _one_at_a_time = i18n::TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let not_found = Failure::Status(404);

        i18n::set(Language::French);
        let french = text(&not_found);
        i18n::set(Language::English);
        let english = text(&not_found);

        assert_eq!((french.as_str(), english.as_str()), ("fichier introuvable (404)", "file not found (404)"));
    }

    #[test]
    fn statuses_without_a_sentence_of_their_own_say_their_number() {
        let _one_at_a_time = i18n::TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        i18n::set(Language::English);

        assert_eq!(text(&Failure::Status(503)), "server error (503)");
        assert_eq!(text(&Failure::Status(418)), "the server answered 418");
    }

    #[test]
    fn a_failure_saved_by_an_older_rdm_reads_as_it_was_written() {
        assert_eq!(text(&Failure::Text("lien expiré (410)".into())), "lien expiré (410)");
    }
}
