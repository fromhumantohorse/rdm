//! Why looking for, downloading or installing an update failed, said in the interface language.

use std::fmt;

use crate::{tr, trf};

#[cfg_attr(not(target_os = "linux"), allow(dead_code, reason = "some installation failures happen on Linux only"))]
#[derive(Debug)]
pub enum UpdateError {
    /// The build does not say where its releases live (`repository` in Cargo.toml).
    NotConfigured,
    Unreachable,
    /// GitHub's API answered with this HTTP status.
    Refused(u16),
    /// GitHub's download servers answered with this HTTP status.
    DownloadRefused(u16),
    UnexpectedAnswer,
    /// A release asset outside GitHub's download servers.
    UnexpectedAddress,
    Unsigned,
    SignatureUnavailable,
    BadSignature,
    TooLarge,
    Stalled,
    Interrupted,
    Incomplete,
    /// The package's SHA-256 differs from the one GitHub published.
    Corrupted,
    /// The file is not the kind of package its name says.
    NotThePackage,
    /// The package was swapped between its check and its installation.
    ChangedAfterVerification,
    /// The installation assistant (Windows) or the password prompt (Linux) could not be started.
    CannotStart(std::io::Error),
    PasswordRefused,
    /// The package manager ended with this code.
    PackageManagerFailed(i32),
    InstallationInterrupted,
    UnknownLocation,
    UnreadableArchive,
    /// This copy of RDM cannot update itself (see `update::Method`).
    NotHere,
    Io(std::io::Error),
}

impl UpdateError {
    /// Worth trying the download again (the network, GitHub's servers); not a refusal, a bad
    /// signature or a full disk.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Unreachable | Self::Stalled | Self::Interrupted | Self::Incomplete | Self::Corrupted | Self::NotThePackage => true,
            Self::Refused(status) | Self::DownloadRefused(status) => *status == 429 || (500..600).contains(status),
            _ => false,
        }
    }
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::NotConfigured => tr!("dépôt GitHub non configuré", "GitHub repository not configured").to_owned(),
            Self::Unreachable => tr!("GitHub est injoignable", "GitHub cannot be reached").to_owned(),
            Self::Refused(status) => trf!("GitHub a répondu {status}", "GitHub answered {status}", status = status),
            Self::DownloadRefused(status) => {
                trf!("téléchargement refusé par GitHub ({status})", "download refused by GitHub ({status})", status = status)
            }
            Self::UnexpectedAnswer => tr!("réponse inattendue de GitHub", "unexpected answer from GitHub").to_owned(),
            Self::UnexpectedAddress => tr!("adresse de téléchargement inattendue", "unexpected download address").to_owned(),
            Self::Unsigned => tr!("mise à jour non signée : installation refusée", "unsigned update: installation refused").to_owned(),
            Self::SignatureUnavailable => tr!("signature de la mise à jour introuvable", "cannot fetch the update's signature").to_owned(),
            Self::BadSignature => {
                tr!("signature de la mise à jour invalide : installation refusée", "invalid update signature: installation refused").to_owned()
            }
            Self::TooLarge => tr!("paquet anormalement gros", "abnormally large package").to_owned(),
            Self::Stalled => tr!("téléchargement bloqué (connexion)", "download stalled (connection)").to_owned(),
            Self::Interrupted => tr!("téléchargement interrompu", "download interrupted").to_owned(),
            Self::Incomplete => tr!("paquet incomplet", "incomplete package").to_owned(),
            Self::Corrupted => tr!("paquet corrompu (empreinte SHA-256 différente)", "corrupted package (SHA-256 mismatch)").to_owned(),
            Self::NotThePackage => tr!("le fichier reçu n'est pas le paquet attendu", "the file received is not the expected package").to_owned(),
            Self::ChangedAfterVerification => tr!(
                "le paquet a été modifié après sa vérification : installation refusée",
                "the package changed after it was verified: installation refused"
            )
            .to_owned(),
            Self::CannotStart(e) => trf!("impossible de lancer l'installation : {e}", "cannot start the installation: {e}", e = e),
            Self::PasswordRefused => tr!("mot de passe refusé ou demande fermée", "password refused or prompt closed").to_owned(),
            Self::PackageManagerFailed(c) => trf!("le gestionnaire de paquets a échoué (code {c})", "the package manager failed (code {c})", c = c),
            Self::InstallationInterrupted => tr!("installation interrompue", "installation interrupted").to_owned(),
            Self::UnknownLocation => tr!("emplacement de RDM inconnu", "RDM's location is unknown").to_owned(),
            Self::UnreadableArchive => tr!("archive de mise à jour illisible", "unreadable update archive").to_owned(),
            Self::NotHere => tr!("mise à jour impossible ici", "cannot update this copy").to_owned(),
            Self::Io(e) => trf!("écriture impossible : {e}", "cannot write: {e}", e = e),
        };
        f.write_str(&text)
    }
}

impl std::error::Error for UpdateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) | Self::CannotStart(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for UpdateError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_network_and_githubs_servers_are_tried_again_refusals_are_not() {
        for retried in [UpdateError::Unreachable, UpdateError::Stalled, UpdateError::Corrupted, UpdateError::Refused(503), UpdateError::Refused(429)] {
            assert!(retried.is_retryable(), "{retried:?}");
        }
        for given_up in [UpdateError::Refused(404), UpdateError::BadSignature, UpdateError::TooLarge, UpdateError::Io(std::io::ErrorKind::StorageFull.into())] {
            assert!(!given_up.is_retryable(), "{given_up:?}");
        }
    }

    #[test]
    fn a_disk_error_keeps_its_cause() {
        let error = UpdateError::from(std::io::Error::from(std::io::ErrorKind::PermissionDenied));

        assert!(std::error::Error::source(&error).is_some());
    }
}
