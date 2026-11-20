use gtk::glib;
use tdlib::types::File;
use FileStatus::*;

#[derive(Debug, Clone, Copy, Default, PartialEq, glib::Boxed)]
#[boxed_type(name = "FileStatus")]
pub(crate) enum FileStatus {
    Downloading(f64),
    Uploading(f64),
    #[default]
    CanBeDownloaded,
    Downloaded,
}

impl From<&File> for FileStatus {
    fn from(file: &File) -> Self {
        let local = &file.local;
        let remote = &file.remote;

        let size = file.size.max(file.expected_size) as u64;

        if local.is_downloading_active {
            let progress = local.downloaded_size as f64 / size as f64;
            Downloading(progress)
        } else if remote.is_uploading_active {
            let progress = remote.uploaded_size as f64 / size as f64;
            Uploading(progress)
        } else if local.is_downloading_completed {
            Downloaded
        } else {
            // TDLib also reports files that cannot be downloaded (yet), e.g.
            // an upload that was interrupted or a file that is not available
            // anymore. They are offered for download like the others: a
            // failing download is reported by TDLib instead of aborting the
            // whole application here.
            if !local.can_be_downloaded {
                log::debug!("File {} cannot be downloaded", file.id);
            }
            CanBeDownloaded
        }
    }
}

#[cfg(test)]
mod tests {
    use tdlib::types::LocalFile;
    use tdlib::types::RemoteFile;

    use super::*;

    fn file() -> File {
        File {
            id: 1,
            size: 1000,
            expected_size: 1000,
            local: LocalFile {
                path: String::new(),
                can_be_downloaded: true,
                can_be_deleted: false,
                is_downloading_active: false,
                is_downloading_completed: false,
                download_offset: 0,
                downloaded_prefix_size: 0,
                downloaded_size: 0,
            },
            remote: RemoteFile {
                id: String::new(),
                unique_id: String::new(),
                is_uploading_active: false,
                is_uploading_completed: true,
                uploaded_size: 1000,
            },
        }
    }

    #[test]
    fn statuses_of_files() {
        assert_eq!(FileStatus::from(&file()), CanBeDownloaded);

        let mut downloading = file();
        downloading.local.is_downloading_active = true;
        downloading.local.downloaded_size = 250;
        assert_eq!(FileStatus::from(&downloading), Downloading(0.25));

        let mut uploading = file();
        uploading.remote.is_uploading_active = true;
        uploading.remote.uploaded_size = 500;
        assert_eq!(FileStatus::from(&uploading), Uploading(0.5));

        let mut downloaded = file();
        downloaded.local.is_downloading_completed = true;
        assert_eq!(FileStatus::from(&downloaded), Downloaded);
    }

    #[test]
    fn files_that_cannot_be_downloaded_do_not_abort() {
        let mut unavailable = file();
        unavailable.local.can_be_downloaded = false;
        assert_eq!(FileStatus::from(&unavailable), CanBeDownloaded);
    }

    #[test]
    fn progress_uses_the_expected_size_when_the_size_is_unknown() {
        let mut downloading = file();
        downloading.size = 0;
        downloading.expected_size = 2000;
        downloading.local.is_downloading_active = true;
        downloading.local.downloaded_size = 500;
        assert_eq!(FileStatus::from(&downloading), Downloading(0.25));
    }
}
