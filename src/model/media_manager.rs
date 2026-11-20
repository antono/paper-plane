//! The `MediaManager` is responsible for deciding whether media should be
//! downloaded automatically and for performing downloads requested by the
//! user (either manually or as a result of the auto-download settings).

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::config;
use crate::model;

/// The kind of media that can be downloaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MediaType {
    /// A photo.
    Photo,
    /// A video or an animation (GIF).
    Video,
    /// A regular file.
    File,
}

/// The auto-download preferences, as stored in the application settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AutoDownloadSettings {
    /// The master switch, `auto-download-media`.
    pub(crate) enabled: bool,
    pub(crate) photos: bool,
    pub(crate) videos: bool,
    pub(crate) videos_max_size: u64,
    pub(crate) files: bool,
    pub(crate) files_max_size: u64,
}

impl AutoDownloadSettings {
    pub(crate) fn from_settings(settings: &gio::Settings) -> Self {
        Self {
            enabled: settings.boolean("auto-download-media"),
            photos: settings.boolean("auto-download-photos"),
            videos: settings.boolean("auto-download-videos"),
            videos_max_size: settings.uint("auto-download-videos-max-size").into(),
            files: settings.boolean("auto-download-files"),
            files_max_size: settings.uint("auto-download-files-max-size").into(),
        }
    }

    /// Returns whether media of the given `kind` and `size` should be
    /// downloaded automatically.
    ///
    /// When the master switch is disabled, nothing is downloaded
    /// automatically. When the per-content switch is disabled, or when the
    /// file is too large for the configured limit (videos and files only),
    /// the media is not downloaded automatically either.
    pub(crate) fn allows(&self, kind: MediaType, size: u64) -> bool {
        if !self.enabled {
            return false;
        }

        match kind {
            MediaType::Photo => self.photos,
            MediaType::Video => self.videos && size <= self.videos_max_size,
            MediaType::File => self.files && size <= self.files_max_size,
        }
    }
}

/// The media download manager.
///
/// It holds a reference to the [`ClientStateSession`] so that it can download
/// files on demand. The auto-download decisions are based on the global
/// application settings and are evaluated on every call, so they always
/// reflect the current settings.
#[derive(Debug)]
pub(crate) struct MediaManager {
    session: glib::WeakRef<model::ClientStateSession>,
    settings: gio::Settings,
}

impl MediaManager {
    /// Creates a new `MediaManager` for the given session.
    pub(crate) fn new(session: &model::ClientStateSession) -> Self {
        let session_ref = glib::WeakRef::new();
        session_ref.set(Some(session));

        Self {
            session: session_ref,
            settings: gio::Settings::new(config::APP_ID),
        }
    }

    /// Returns whether media of the given `kind` and `size` should be
    /// downloaded automatically.
    ///
    /// When the master `auto-download-media` switch is disabled, nothing is
    /// downloaded automatically. When the per-content switch is disabled, or
    /// when the file is too large for the configured limit (videos and files
    /// only), the media is not downloaded automatically either.
    pub(crate) fn should_auto_download(&self, kind: MediaType, size: u64) -> bool {
        AutoDownloadSettings::from_settings(&self.settings).allows(kind, size)
    }

    /// Downloads the file of the given `id`, calling `f` every time there's an
    /// update about the progress or when the download has completed.
    ///
    /// This is used to download files requested by the user (e.g. when they
    /// click on a media preview that should not be downloaded automatically).
    pub(crate) fn download_file_with_updates<F: Fn(tdlib::types::File) + 'static>(
        &self,
        file_id: i32,
        f: F,
    ) {
        if let Some(session) = self.session.upgrade() {
            session.download_file_with_updates(file_id, f);
        }
    }

    /// Cancels the download of the file of the given `id`.
    pub(crate) fn cancel_download_file(&self, file_id: i32) {
        if let Some(session) = self.session.upgrade() {
            session.cancel_download_file(file_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;

    fn all_enabled() -> AutoDownloadSettings {
        AutoDownloadSettings {
            enabled: true,
            photos: true,
            videos: true,
            videos_max_size: 10 * MB,
            files: true,
            files_max_size: MB,
        }
    }

    #[test]
    fn master_switch_disables_everything() {
        let settings = AutoDownloadSettings {
            enabled: false,
            ..all_enabled()
        };
        for kind in [MediaType::Photo, MediaType::Video, MediaType::File] {
            assert!(!settings.allows(kind, 0), "{kind:?}");
        }
    }

    #[test]
    fn per_kind_switches() {
        let settings = all_enabled();
        assert!(settings.allows(MediaType::Photo, 100 * MB));
        assert!(!AutoDownloadSettings {
            photos: false,
            ..settings
        }
        .allows(MediaType::Photo, 0));
        assert!(!AutoDownloadSettings {
            videos: false,
            ..settings
        }
        .allows(MediaType::Video, 0));
        assert!(!AutoDownloadSettings {
            files: false,
            ..settings
        }
        .allows(MediaType::File, 0));
    }

    #[test]
    fn size_limits_are_inclusive() {
        let settings = all_enabled();
        assert!(settings.allows(MediaType::Video, 10 * MB));
        assert!(!settings.allows(MediaType::Video, 10 * MB + 1));
        assert!(settings.allows(MediaType::File, MB));
        assert!(!settings.allows(MediaType::File, MB + 1));
    }

    /// The keys read by `from_settings` exist in the schema with the
    /// expected types. The compiled schema of the build directory is used
    /// when `PAPER_PLANE_SCHEMA_DIR` is set (meson test sets it).
    #[test]
    fn settings_schema_has_the_auto_download_keys() {
        let Some(dir) = std::env::var_os("PAPER_PLANE_SCHEMA_DIR") else {
            eprintln!("PAPER_PLANE_SCHEMA_DIR is not set: skipping");
            return;
        };
        let source = gio::SettingsSchemaSource::from_directory(
            dir,
            None::<&gio::SettingsSchemaSource>,
            false,
        )
        .expect("compiled schemas");
        let schema = source
            .lookup(config::APP_ID, false)
            .expect("the application schema");

        for (key, type_) in [
            ("auto-download-media", "b"),
            ("auto-download-photos", "b"),
            ("auto-download-videos", "b"),
            ("auto-download-videos-max-size", "u"),
            ("auto-download-files", "b"),
            ("auto-download-files-max-size", "u"),
            ("run-in-background", "b"),
        ] {
            assert!(schema.has_key(key), "missing key {key}");
            assert_eq!(schema.key(key).value_type().as_str(), type_, "{key}");
        }

        let settings = gio::Settings::new_full(&schema, None::<&gio::SettingsBackend>, None);
        let _ = AutoDownloadSettings::from_settings(&settings);
    }
}
