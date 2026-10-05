//! The settings and the saved passwords, as the service applies them.

use super::*;

impl Manager {
    /// The saved passwords (site logins, proxy), for the settings.
    pub fn secrets(&self) -> Secrets {
        lock(&self.secrets).clone()
    }

    /// Replaces the saved passwords; downloads started from now on use them.
    pub fn set_secrets(&self, secrets: Secrets) {
        let changed = {
            let mut current = lock(&self.secrets);
            let changed = *current != secrets;
            *current = secrets;
            changed
        };
        if changed {
            let saved = lock(&self.secrets).save();
            self.saved(saved);
            self.forget_clients(); // the proxy password may have changed
        }
    }

    pub fn settings(&self) -> Settings {
        lock(&self.settings).clone()
    }

    /// One field without cloning the whole settings (read every frame).
    pub fn with_settings<R>(&self, f: impl FnOnce(&Settings) -> R) -> R {
        f(&lock(&self.settings))
    }

    /// Takes effect immediately; `save_settings` persists (the UI debounces it, shutdown flushes it).
    pub fn apply_settings(self: &Arc<Self>, mut new: Settings) {
        new.sanitize();
        self.limit.set(u64::from(new.speed_limit_kib) * 1024);
        crate::i18n::set(new.language);
        let (proxy_changed, queues) = {
            let mut current = lock(&self.settings);
            let proxy_changed = current.proxy != new.proxy;
            let queues: HashSet<u32> = new.queues.iter().map(|q| q.id).collect();
            *current = new;
            (proxy_changed, queues)
        };
        if proxy_changed {
            self.forget_clients();
        }
        // Downloads of a deleted queue go back to the main one.
        let mut moved = false;
        for e in lock(&self.entries).iter_mut() {
            if e.download.queue != 0 && !queues.contains(&e.download.queue) {
                e.download.queue = 0;
                moved = true;
            }
        }
        if moved {
            self.changed();
        }
        self.settings_dirty.store(true, Release);
        self.schedule();
    }

    /// Writes the settings if they changed since the last save (tried again next time if it fails).
    pub fn save_settings(&self) {
        if self.settings_dirty.swap(false, AcqRel) {
            let saved = lock(&self.settings).save(); // under the lock: saves land in the order of the changes
            if saved.is_err() {
                self.settings_dirty.store(true, Release);
            }
            self.saved(saved);
        }
    }
}
