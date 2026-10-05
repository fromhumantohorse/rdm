//! "Advanced": performance, proxy, updates.

use super::*;

pub(super) fn advanced_tab(ui: &mut Ui, p: &Palette, s: &mut Settings, secrets: &mut Secrets, update_state: &update::State) -> Option<FormAction> {
    let mut action = None;
    widgets::icon_text(
        ui,
        icon::WARNING,
        p.warning,
        tr!(
            "Les valeurs par défaut conviennent à la plupart des gens : ne les changez que si vous savez pourquoi.",
            "The default values suit most people: change these only if you know why."
        ),
        p.muted,
        12.5,
    );
    ui.add_space(10.0);
    section(ui, p, icon::LIGHTNING, tr!("Performances", "Performance"), |ui| {
        ui.add(Slider::new(&mut s.connections, 1..=MAX_CONNECTIONS).text(tr!("connexions par fichier (au plus)", "connections per file (at most)")));
        ui.add(Slider::new(&mut s.max_parallel, 1..=MAX_PARALLEL).text(tr!("téléchargements simultanés (file principale)", "simultaneous downloads (main queue)")));
        note(
            ui,
            p,
            tr!(
                "Plus de connexions peuvent accélérer un gros fichier ; certains sites en refusent trop.",
                "More connections can speed up a large file; some sites refuse too many."
            ),
        );
        note(
            ui,
            p,
            tr!(
                "RDM adapte seul le nombre de connexions à la connexion et au serveur : ce réglage n'est qu'un plafond.",
                "RDM adapts the number of connections to the network and the server by itself: this setting is only a ceiling."
            ),
        );
    });

    if let Some(test) = proxy_section(ui, p, s, secrets) {
        action = Some(test);
    }

    section(ui, p, icon::ROCKET_LAUNCH, tr!("Mises à jour", "Updates"), |ui| {
        toggle(
            ui,
            &mut s.check_updates,
            tr!("Rechercher automatiquement", "Check automatically"),
            tr!("Au démarrage puis une fois par jour, sur GitHub (une seule requête)", "At start then once a day, on GitHub (a single request)"),
        );
        let silent = matches!(update::method(), update::Method::Msi | update::Method::Binary);
        if silent {
            toggle(
                ui,
                &mut s.auto_update,
                tr!("Installer automatiquement", "Install automatically"),
                tr!("En silence, dès qu'aucun téléchargement n'est en cours ; RDM redémarre tout seul", "Silently, as soon as nothing is downloading; RDM restarts by itself"),
            );
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.add_enabled_ui(!update_state.busy(), |ui| ghost_button(ui, icon::ARROWS_CLOCKWISE, tr!("Rechercher maintenant", "Check now"))).inner.clicked() {
                action = Some(FormAction::Window(Action::CheckUpdates));
            }
            let version = env!("CARGO_PKG_VERSION");
            let (glyph, text, color) = match update_state {
                update::State::Idle => (icon::INFO, trf!("Version installée : {version}", "Installed version: {version}", version = version), p.muted),
                update::State::Checking => (icon::CIRCLE_NOTCH, tr!("Recherche en cours…", "Checking…").to_owned(), p.accent),
                update::State::UpToDate => (icon::CHECK_CIRCLE, trf!("RDM est à jour ({version})", "RDM is up to date ({version})", version = version), p.success),
                update::State::Available(r) => (icon::ROCKET_LAUNCH, trf!("Version {} disponible", "Version {} available", r.version), p.accent),
                update::State::Downloading(f) => {
                    let percent = (f * 100.0) as u32;
                    (icon::DOWNLOAD_SIMPLE, trf!("Téléchargement {percent} %", "Downloading {percent}%", percent = percent), p.accent)
                }
                update::State::Installing => (icon::ROCKET_LAUNCH, tr!("Installation : RDM redémarre tout seul", "Installing: RDM restarts by itself").to_owned(), p.accent),
                update::State::InstallFailed(r, reason) => (icon::WARNING, trf!("Version {} : {reason}", "Version {}: {reason}", r.version, reason = reason), p.danger),
                update::State::Failed(reason) => (icon::WARNING, reason.clone(), p.danger),
            };
            ui.add(Label::new(RichText::new(format!("{glyph}  {text}")).font(theme::regular(13.0)).color(color)).wrap());
        });
        if let Some(r) = update_state.release() {
            ui.add_space(6.0);
            let label = match (update::installs_itself(r), update_state) {
                (true, update::State::InstallFailed(..)) => tr!("Réessayer l'installation", "Retry the installation"),
                (true, _) => tr!("Installer la mise à jour", "Install the update"),
                (false, _) => tr!("Voir la nouvelle version", "See the new version"),
            };
            if accent_button(ui, icon::ROCKET_LAUNCH, label).clicked() {
                action = Some(FormAction::Window(Action::InstallUpdate));
            }
        }
    });
    action
}

/// The proxy: none, the system's, always this one, or this one as a fallback; `TestProxy` when
/// the user asks to try it.
fn proxy_section(ui: &mut Ui, p: &Palette, s: &mut Settings, secrets: &mut Secrets) -> Option<FormAction> {
    let mut action = None;
    section(ui, p, icon::GLOBE_HEMISPHERE_WEST, tr!("Connexion à Internet (proxy)", "Internet connection (proxy)"), |ui| {
        note(
            ui,
            p,
            tr!(
                "Un proxy est un serveur intermédiaire par lequel passent les téléchargements (réseau d'entreprise, école…). Dans le doute, gardez « Système ».",
                "A proxy is a go-between server that downloads pass through (company or school network…). If unsure, keep \"System\"."
            ),
        );
        ui.add_space(8.0);
        segmented(
            ui,
            &mut s.proxy.mode,
            &[
                (ProxyMode::Off, icon::PLUGS_CONNECTED, tr!("Aucun", "None")),
                (ProxyMode::System, icon::DESKTOP, tr!("Système", "System")),
                (ProxyMode::Manual, icon::SHIELD, tr!("Toujours", "Always")),
                (ProxyMode::Auto, icon::MAGIC_WAND, tr!("En secours", "As a fallback")),
            ],
        );
        note(
            ui,
            p,
            match s.proxy.mode {
                ProxyMode::Off => tr!(
                    "Aucun proxy : RDM se connecte directement aux sites, même si le système en indique un.",
                    "No proxy: RDM connects to the sites directly, even if the system names one."
                ),
                ProxyMode::System => tr!(
                    "Recommandé. RDM utilise le proxy réglé dans le système, s'il y en a un ; sinon il se connecte directement.",
                    "Recommended. RDM uses the proxy set in the system, if there is one; otherwise it connects directly."
                ),
                ProxyMode::Manual => tr!(
                    "Tous les téléchargements passent par le proxy que vous indiquez ci-dessous.",
                    "Every download goes through the proxy you enter below."
                ),
                ProxyMode::Auto => tr!(
                    "Connexion directe d'abord ; si un téléchargement est bloqué ou très lent, RDM passe tout seul par le proxy ci-dessous.",
                    "Direct connection first; if a download is blocked or very slow, RDM switches to the proxy below by itself."
                ),
            },
        );
        if matches!(s.proxy.mode, ProxyMode::Manual | ProxyMode::Auto) {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                row_label(ui, p, tr!("Adresse", "Address"));
                ui.add(text_field(&mut s.proxy.url, "socks5h://host:1080 · http://host:3128", p, ui.available_width()).font(eframe::egui::TextStyle::Monospace));
            });
            ui.horizontal(|ui| {
                row_label(ui, p, tr!("Identifiant", "User"));
                ui.add(text_field(&mut s.proxy.user, tr!("(facultatif)", "(optional)"), p, 180.0));
                ui.add(text_field(&mut secrets.proxy_password, tr!("mot de passe", "password"), p, 180.0).password(true));
            });
            note(
                ui,
                p,
                tr!(
                    "HTTP, HTTPS, SOCKS5 et SOCKS4a. Avec socks5h:// ou socks4a://, les noms sont résolus par le proxy (aucune requête DNS locale).",
                    "HTTP, HTTPS, SOCKS5 and SOCKS4a. With socks5h:// or socks4a://, names are resolved by the proxy (no local DNS request)."
                ),
            );
        }
        if s.proxy.mode != ProxyMode::Off {
            ui.add_space(4.0);
            if ghost_button(ui, icon::PLAY, tr!("Tester la connexion", "Test the connection")).clicked() {
                action = Some(FormAction::TestProxy);
            }
        }
    });
    action
}
