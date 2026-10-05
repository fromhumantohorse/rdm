//! What the list shows: the sidebar's filters (status, category, queue) and their names.

use domain::{Category, Status};
use egui_phosphor::regular as icon;

use crate::{manager::Entry, settings::Queue, tr, trf};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Filter {
    All,
    Active,
    Queued,
    Done,
    Failed,
    Kind(Category),
    /// A named queue (0 = the main one).
    Queue(u32),
}

impl Filter {
    pub(super) fn accepts(self, e: &Entry) -> bool {
        let status = e.download.status();
        match self {
            Self::All => true,
            Self::Active => *status == Status::Running,
            Self::Queued => matches!(status, Status::Queued | Status::Paused),
            Self::Done => *status == Status::Completed,
            Self::Failed => matches!(status, Status::Failed(_)),
            Self::Kind(c) => e.category == c,
            Self::Queue(q) => e.download.queue == q && !e.download.is_recording(),
        }
    }

    pub(super) fn title(self, queues: &[Queue]) -> String {
        match self {
            Self::All => tr!("Tous les téléchargements", "All downloads").into(),
            Self::Active => tr!("En cours", "Active").into(),
            Self::Queued => tr!("En attente", "Waiting").into(),
            Self::Done => tr!("Terminés", "Completed").into(),
            Self::Failed => tr!("Échecs", "Failed").into(),
            Self::Kind(c) => crate::i18n::category(c).into(),
            Self::Queue(q) => queue_name(queues, q),
        }
    }
}

/// A queue's name as shown (the main queue has no name of its own).
pub(super) fn queue_name(queues: &[Queue], queue: u32) -> String {
    match queues.iter().find(|q| q.id == queue) {
        Some(q) if !q.name.is_empty() => q.name.clone(),
        Some(q) => {
            let id = q.id;
            trf!("File {id}", "Queue {id}", id = id)
        }
        None => tr!("File principale", "Main queue").into(),
    }
}

/// Sidebar: the status filters, then one entry per category.
pub(super) const STATUS_FILTERS: usize = 5;
pub(super) const NAV: [(Filter, &str); STATUS_FILTERS + Category::ALL.len()] = [
    (Filter::All, icon::STACK),
    (Filter::Active, icon::ARROW_CIRCLE_DOWN),
    (Filter::Queued, icon::HOURGLASS),
    (Filter::Done, icon::CHECK_CIRCLE),
    (Filter::Failed, icon::WARNING_CIRCLE),
    (Filter::Kind(Category::ALL[0]), category_icon(Category::ALL[0])),
    (Filter::Kind(Category::ALL[1]), category_icon(Category::ALL[1])),
    (Filter::Kind(Category::ALL[2]), category_icon(Category::ALL[2])),
    (Filter::Kind(Category::ALL[3]), category_icon(Category::ALL[3])),
    (Filter::Kind(Category::ALL[4]), category_icon(Category::ALL[4])),
    (Filter::Kind(Category::ALL[5]), category_icon(Category::ALL[5])),
];

pub(super) const fn category_icon(c: Category) -> &'static str {
    match c {
        Category::Video => icon::FILM_STRIP,
        Category::Music => icon::MUSIC_NOTES,
        Category::Archive => icon::FILE_ZIP,
        Category::Program => icon::APP_WINDOW,
        Category::Document => icon::FILE_TEXT,
        Category::Other => icon::FILE,
    }
}
