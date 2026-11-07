//! What the user does to the downloads of the list: pause, resume, remove, a new link, a speed cap,
//! another queue, a certificate exemption.

use super::*;

impl Manager {
    /// ▶ on a paused / failed download (or one waiting to retry): back in the queue, now.
    pub fn resume(self: &Arc<Self>, id: DownloadId) {
        self.update(id, Entry::resume);
        self.schedule();
    }

    pub fn pause(self: &Arc<Self>, id: DownloadId) {
        self.update(id, Entry::pause);
        self.schedule();
    }

    /// One pass under one lock (queued downloads are paused too: nothing starts afterwards).
    pub fn pause_all(&self) {
        lock(&self.entries).iter_mut().for_each(Entry::pause);
        self.changed();
    }

    /// Paused and failed downloads go back in the queue, in their list order.
    pub fn resume_all(self: &Arc<Self>) {
        lock(&self.entries).iter_mut().for_each(Entry::resume);
        self.changed();
        self.schedule();
    }

    /// Removes from the list; `delete_file` also erases the file (always for unfinished parts).
    pub fn remove(self: &Arc<Self>, id: DownloadId, delete_file: bool) {
        let mut naming = lock(&self.naming);
        if let Some(token) = self.token_of(id) {
            self.cancel_recording(&token, Failure::RecordingCancelled);
        }
        let removed = {
            let mut entries = lock(&self.entries);
            entries.iter().position(|e| e.download.id == id).map(|i| entries.remove(i))
        };
        if let Some(e) = &removed
            && (delete_file || *e.download.status() != Status::Completed)
        {
            naming.push(files::reserved(e));
        }
        drop(naming);
        if let Some(e) = removed {
            if let Some(cancel) = &e.cancel {
                cancel.cancel();
            }
            if delete_file || *e.download.status() != Status::Completed {
                let this = self.clone();
                let (target, replaces) = (e.download.target.clone(), e.replaces);
                self.rt.spawn(async move {
                    // Cancellation can take longer than a network timeout or a slow disk write.
                    // Never erase under a live writer, nor let a new entry reuse its name.
                    while lock(&this.busy).contains(&id) {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    for path in leftovers(&target, delete_file, replaces) {
                        let _ = tokio::fs::remove_file(path).await;
                    }
                    remove_recording_leftovers(&target).await;
                    lock(&this.naming).retain(|entry| entry.id != id);
                });
            }
        }
        self.changed();
        self.schedule();
    }

    pub fn clear_completed(&self) {
        lock(&self.entries).retain(|e| *e.download.status() != Status::Completed);
        self.changed();
    }

    /// SHA-256 of a finished file, computed off the UI thread, then shown by the UI.
    pub fn compute_sha256(self: &Arc<Self>, id: DownloadId) {
        let Some(path) = self.view(|es| {
            es.iter().find(|e| e.download.id == id && *e.download.status() == Status::Completed).map(|e| e.download.target.clone())
        }) else {
            return;
        };
        let this = self.clone();
        self.rt.spawn_blocking(move || {
            if let Ok(hash) = checksum::digest(&path, domain::HashAlgo::Sha256) {
                this.update(id, |e| e.sha256 = Some(hash));
            }
        });
    }

    /// Replaces the link of a download that stopped working (expired, moved) and resumes it where
    /// it was. The new link must lead to the same file: when the server tells its size and it
    /// differs, the change is refused (the parts already downloaded would not fit). A stream (HLS)
    /// or a video fetched with a separate audio track has no single size to compare.
    pub fn change_url(self: &Arc<Self>, id: DownloadId, url: Url) {
        let Some((total, headers, via_proxy, insecure, split)) = self.view(|es| {
            es.iter().find(|e| e.download.id == id).map(|e| {
                (e.progress.total.load(Relaxed), to_header_map(&e.headers), e.via_proxy, e.download.insecure, e.download.audio.is_some())
            })
        }) else {
            return;
        };
        let this = self.clone();
        self.rt.spawn(async move {
            let size = match this.client_for_url(&url, via_proxy, insecure).await {
                Ok(client) => engine::probe_once(&client, &url, &headers).await.ok().filter(|p| !p.hls).and_then(|p| p.size),
                Err(_) => None,
            };
            if !split && total > 0 && size.is_some_and(|s| s != total) {
                let (have, got) = (crate::i18n::bytes(total), crate::i18n::bytes(size.unwrap_or(0)));
                this.notice(true, &trf!(
                    "Lien refusé : le fichier fait {got}, pas {have} (ce n'est pas le même)",
                    "Link refused: the file is {got}, not {have} (not the same file)",
                    got = got,
                    have = have
                ));
                return;
            }
            let changed = this.update(id, |e| {
                if e.download.is_recording() || *e.download.status() == Status::Completed {
                    return false;
                }
                e.download.url = url.clone();
                e.retries = 0;
                e.retry = None;
                match e.download.status() {
                    Status::Running => e.restart(),
                    Status::Failed(_) => {
                        let _ = e.download.enqueue();
                    }
                    _ => {}
                }
                true
            });
            if changed == Some(true) {
                this.notice(false, tr!("Lien remplacé : le téléchargement reprend où il en était", "Link replaced: the download resumes where it was"));
                this.schedule();
            }
        });
    }

    /// This download's own speed cap (KiB/s, 0 = none), applied at once if it runs.
    pub fn set_speed_limit(&self, id: DownloadId, kib: u32) {
        self.update(id, |e| {
            e.download.speed_limit_kib = kib;
            e.own_limit.set(u64::from(kib) * 1024);
        });
    }

    pub fn move_to_queue(self: &Arc<Self>, id: DownloadId, queue: u32) {
        self.update(id, |e| e.download.queue = queue);
        self.schedule();
    }

    /// Accepts (or not) an invalid TLS certificate for this download only, and retries it.
    pub fn set_insecure(self: &Arc<Self>, id: DownloadId, insecure: bool) {
        self.update(id, |e| {
            e.download.insecure = insecure;
            match e.download.status() {
                Status::Running => e.restart(),
                Status::Failed(_) if insecure => {
                    let _ = e.download.enqueue();
                }
                _ => {}
            }
        });
        self.schedule();
    }
}
