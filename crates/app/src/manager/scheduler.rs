//! Starting queued downloads, and what the end of each run means: completed, retried later on its
//! own, or failed.

use super::*;

/// Transient failures (network down, server busy) are retried on their own this many times in a
/// row without progress — about half an hour — before the download is reported as failed.
const AUTO_RETRIES: u32 = 15;

/// 5 s, 10 s, 20 s, 40 s, 80 s, then every 2 minutes.
fn retry_delay(retries: u32) -> Duration {
    Duration::from_secs((5u64 << retries.min(5)).min(120))
}

impl Manager {
    /// Starts queued downloads (FIFO) while fewer than `max_parallel` are running. Choosing and
    /// marking them running happen under one lock: concurrent calls can never start one twice nor
    /// exceed the limit.
    pub(super) fn schedule(self: &Arc<Self>) {
        if self.is_closing() {
            return;
        }
        // Each queue has its own number of places; a download of an unknown queue counts as the
        // main queue's.
        let (limits, main_limit) = self.with_settings(|s| {
            let limits: HashMap<u32, usize> = s.queues.iter().map(|q| (q.id, usize::from(q.max_parallel))).collect();
            (limits, usize::from(s.max_parallel))
        });
        let queue_of = |q: u32| if limits.contains_key(&q) { q } else { 0 };
        let limit_of = |q: u32| limits.get(&q).copied().unwrap_or(main_limit);
        let now = Instant::now();
        let launches: Vec<Launch> = {
            let mut entries = lock(&self.entries);
            let mut busy = lock(&self.busy);
            let mut running: HashMap<u32, usize> = HashMap::new();
            for e in entries.iter().filter(|e| e.occupies_slot()) {
                *running.entry(queue_of(e.download.queue)).or_default() += 1;
            }
            let mut launches = Vec::new();
            for e in entries.iter_mut() {
                if !e.startable(now) || busy.contains(&e.download.id) {
                    continue;
                }
                let queue = queue_of(e.download.queue);
                let taken = running.entry(queue).or_default();
                if *taken >= limit_of(queue) {
                    continue;
                }
                if let Some(launch) = e.start(&self.limit) {
                    busy.insert(launch.id);
                    launches.push(launch);
                    *taken += 1;
                }
            }
            launches
        };
        if launches.is_empty() {
            return;
        }
        self.changed();
        launches.into_iter().for_each(|launch| self.run(launch));
    }

    fn run(self: &Arc<Self>, launch: Launch) {
        let Launch { id, mut job, progress, cancel, via_proxy, insecure } = launch;
        self.inflight.fetch_add(1, AcqRel);
        let this = self.clone();
        self.rt.spawn(async move {
            this.add_login(&job.url, &mut job.headers);
            let before = progress.downloaded.load(Relaxed);
            let result = match this.client_for_url(&job.url, via_proxy, insecure).await {
                Ok(client) => Ok(engine::run(&client, &job, progress.clone(), cancel).await),
                Err(reason) => Err(reason),
            };
            let progressed = progress.downloaded.load(Relaxed) > before;
            if matches!(result, Ok(Ok(Outcome::Completed))) {
                // Marked before the list shows it complete (and can open it).
                let target = job.target.clone();
                let _ = tokio::task::spawn_blocking(move || mark_from_internet(&target)).await;
            }
            let auto_proxy = this.auto_proxy();
            let (mut finished, mut verify) = (None, false);
            this.update(id, |e| {
                e.cancel = None;
                let restart = std::mem::take(&mut e.restart);
                if progressed {
                    e.retries = 0;
                }
                let _ = match &result {
                    Err(RouteError::InvalidProxy) => e.download.fail(Failure::InvalidProxy),
                    Ok(Ok(Outcome::Completed)) => {
                        let _ = e.download.start(); // a pause may have raced the last byte
                        finished = Some(e.name.clone());
                        verify = e.download.checksum.is_some();
                        e.retries = 0;
                        e.forget_secrets();
                        e.download.complete()
                    }
                    // Stopped to start again (new route, link, certificate choice): straight back.
                    Ok(Ok(Outcome::Paused)) if restart => e.download.retry_later(),
                    Ok(Ok(Outcome::Paused)) => Ok(()),
                    // Network down, server busy: back in the queue, retried later on its own.
                    Ok(Err(err)) if !err.is_permanent() && e.retries < AUTO_RETRIES && e.download.retry_later().is_ok() => {
                        // Automatic proxy mode: a server the direct route cannot reach is tried
                        // through the proxy, at once.
                        let unreachable = matches!(err, engine::EngineError::Http(h) if h.is_connect() || h.is_timeout());
                        let delay = if auto_proxy && !e.via_proxy && unreachable {
                            e.via_proxy = true;
                            Duration::ZERO
                        } else {
                            retry_delay(e.retries)
                        };
                        e.retry = Some(Retry { at: Instant::now() + delay, reason: failure_of(err) });
                        e.retries += 1;
                        Ok(())
                    }
                    Ok(Err(err)) => e.download.fail(failure_of(err)),
                };
            });
            lock(&this.busy).remove(&id);
            this.inflight.fetch_sub(1, AcqRel);
            if verify {
                this.verify(id);
            }
            if let Some(name) = finished
                && this.with_settings(|s| s.notify)
            {
                notify::completed(&name);
            }
            this.schedule();
        });
    }
}

/// What an engine error means for the download (never its URL: it may carry tokens).
fn failure_of(err: &engine::EngineError) -> Failure {
    use engine::EngineError as E;
    match err {
        E::Http(e) if e.is_redirect() => Failure::RedirectRefused,
        _ if err.is_certificate() => Failure::InvalidCertificate,
        E::Http(e) => match e.status() {
            Some(status) => Failure::Status(status.as_u16()),
            None if e.is_timeout() => Failure::TimedOut,
            None if e.is_connect() => Failure::CannotConnect,
            None => Failure::ConnectionInterrupted,
        },
        E::Io(e) => Failure::Disk(e.to_string()),
        E::RangeIgnored | E::FileChanged => Failure::InconsistentRange,
        E::Empty => Failure::NoData,
        E::Truncated => Failure::CutBeforeTheEnd,
        E::Stalled => Failure::ConnectionInterrupted,
        E::LocalNetwork => Failure::LocalNetwork,
        E::Playlist(e) => Failure::Stream(e.to_string()),
        E::Mux(e) => Failure::Merge(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_back_off_from_five_seconds_to_every_two_minutes() {
        let delays: Vec<u64> = (0..8).map(|retries| retry_delay(retries).as_secs()).collect();

        assert_eq!(delays, [5, 10, 20, 40, 80, 120, 120, 120]);
    }

    #[test]
    fn engine_errors_become_the_failure_the_list_explains() {
        use engine::EngineError as E;

        assert_eq!(failure_of(&E::RangeIgnored), Failure::InconsistentRange);
        assert_eq!(failure_of(&E::FileChanged), Failure::InconsistentRange);
        assert_eq!(failure_of(&E::Empty), Failure::NoData);
        assert_eq!(failure_of(&E::Truncated), Failure::CutBeforeTheEnd);
        assert_eq!(failure_of(&E::Stalled), Failure::ConnectionInterrupted);
        assert_eq!(failure_of(&E::LocalNetwork), Failure::LocalNetwork);
        let full = std::io::Error::from(std::io::ErrorKind::StorageFull);
        assert_eq!(failure_of(&E::Io(std::io::Error::from(std::io::ErrorKind::StorageFull))), Failure::Disk(full.to_string()));
    }
}
