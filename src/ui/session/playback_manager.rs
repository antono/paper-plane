//! A shared playback engine that owns a single `gtk::MediaFile` per session.
//!
//! Widgets that play media (the media viewer, and in the future audio
//! message rows) borrow the stream of this manager instead of creating
//! their own, so that at most one media pipeline exists per session.
//!
//! The `MediaFile` is exposed to the clients for read-only inspection
//! (`is_playing()`, `position()`, `duration()`) and for binding it to
//! render targets like `gtk::Video` or `gtk::MediaControls`, as well as
//! for pre-load configuration (`set_muted()`, `set_loop()`). Its source
//! and lifecycle, however, are owned by the manager: clients must not
//! call `set_file()`, `set_filename()`, `clear()`, `play()`, `pause()`
//! or connect their own `prepared`/`error` handlers. Playback control
//! goes through the methods of the manager, and the per-playback hooks
//! passed to `play_file` replace signal handlers.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

/// The hook that is called when the stream of the current playback is
/// prepared.
type OnPrepared = Box<dyn Fn(&gtk::MediaFile)>;

struct Inner {
    /// The shared media stream. Created empty; its source is set by
    /// `play_file`, so no pipeline exists while nothing plays.
    media_file: gtk::MediaFile,
    /// The path of the currently loaded source, if any.
    current_path: RefCell<Option<glib::GString>>,
    /// Resets the UI of the widget that owns the current playback. Run
    /// when another widget takes over or the playback is stopped.
    on_reset_previous: RefCell<Option<Box<dyn Fn()>>>,
    /// Called when the stream of the current playback is prepared, if it
    /// is still current. Set by the widget that started the playback.
    on_prepared: RefCell<Option<OnPrepared>>,
    /// Called when the playback of the current source fails. Set by the
    /// widget that started the playback.
    on_error: RefCell<Option<Box<dyn Fn()>>>,
    /// The `notify::playing` handlers of the widget that owns the current
    /// playback. Disconnected when the playback is taken over or stopped.
    playing_handlers: RefCell<Vec<glib::SignalHandlerId>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        for id in self.playing_handlers.borrow_mut().drain(..) {
            self.media_file.disconnect(id);
        }
    }
}

/// A cloneable handle to the shared playback engine of a session.
#[derive(Clone)]
pub(crate) struct PlaybackManager(Rc<Inner>);

impl fmt::Debug for PlaybackManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PlaybackManager").finish()
    }
}

impl Default for PlaybackManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaybackManager {
    pub(crate) fn new() -> Self {
        let media_file = gtk::MediaFile::new();

        let inner = Rc::new(Inner {
            media_file: media_file.clone(),
            current_path: RefCell::new(None),
            on_reset_previous: RefCell::new(None),
            on_prepared: RefCell::new(None),
            on_error: RefCell::new(None),
            playing_handlers: RefCell::new(Vec::new()),
        });

        media_file.connect_notify_local(
            Some("prepared"),
            glib::clone!(
                #[strong]
                inner,
                move |media: &gtk::MediaFile, _| {
                    let on_prepared = inner.on_prepared.borrow();
                    if media.is_prepared() {
                        if let Some(on_prepared) = on_prepared.as_ref() {
                            on_prepared(media);
                        }
                    }
                }
            ),
        );
        media_file.connect_notify_local(
            Some("error"),
            glib::clone!(
                #[strong]
                inner,
                move |media: &gtk::MediaFile, _| {
                    if let Some(error) = media.error() {
                        log::warn!("Media playback failed: {error:?}");

                        if let Some(on_error) = inner.on_error.borrow().as_ref() {
                            on_error();
                        }
                    }
                }
            ),
        );

        Self(inner)
    }

    /// The shared media stream, for inspection, binding it to a render
    /// target and pre-load configuration. See the module docs for what
    /// clients must not do with it.
    pub(crate) fn media_file(&self) -> gtk::MediaFile {
        self.0.media_file.clone()
    }

    /// Whether the stream is currently loaded from `path`.
    pub(crate) fn is_current(&self, path: &str) -> bool {
        self.0
            .current_path
            .borrow()
            .as_ref()
            .is_some_and(|current| current == path)
    }

    /// Plays the file at `path`, taking the stream over from the previous
    /// playback.
    ///
    /// Before calling this method, the stream may be configured via
    /// `media_file()` (e.g. `set_muted` / `set_loop`), which applies to
    /// the upcoming source.
    ///
    /// The previous playback is stopped: its reset callback is run and
    /// its handlers are disconnected, so that the widget that owned it
    /// can restore its UI.
    ///
    /// `on_prepared` is called when the stream is prepared and its first
    /// frame is available; the playback is *not* started automatically,
    /// the hook decides what to do with the prepared stream. If the
    /// stream is already prepared (e.g. the same source is reattached),
    /// it is called directly, since `notify::prepared` won't fire again.
    ///
    /// `on_error` is called if the playback fails. Note that it can also
    /// be called while another widget owns the stream, so it must not
    /// assume the error belongs to this playback.
    pub(crate) fn play_file<R, P, E>(&self, path: &str, on_reset: R, on_prepared: P, on_error: E)
    where
        R: Fn() + 'static,
        P: Fn(&gtk::MediaFile) + 'static,
        E: Fn() + 'static,
    {
        if let Some(on_reset_previous) = self.0.on_reset_previous.take() {
            on_reset_previous();
        }
        let old_handlers = std::mem::take(&mut *self.0.playing_handlers.borrow_mut());
        for id in old_handlers {
            self.0.media_file.disconnect(id);
        }

        *self.0.on_reset_previous.borrow_mut() = Some(Box::new(on_reset));
        *self.0.on_prepared.borrow_mut() = Some(Box::new(on_prepared));
        *self.0.on_error.borrow_mut() = Some(Box::new(on_error));
        *self.0.current_path.borrow_mut() = Some(glib::GString::from(path));

        let media = &self.0.media_file;
        media.pause();
        media.clear();
        if media.is_prepared() {
            media.stream_unprepared();
        }
        media.invalidate_contents();
        media.set_filename(Some(path));

        if media.is_prepared() {
            let on_prepared = self.0.on_prepared.borrow();
            if let Some(on_prepared) = on_prepared.as_ref() {
                on_prepared(media);
            }
        }
    }

    /// Connects `on_playing` to the `playing` state of the stream on
    /// behalf of the current playback, replacing the connection of the
    /// previous one.
    ///
    /// Must be called by the widget that started the current playback,
    /// ideally right before or right after `play_file`, so that its
    /// connection doesn't get disconnected by a subsequent call.
    pub(crate) fn connect_playing<F: Fn(&gtk::MediaFile) + 'static>(&self, on_playing: F) {
        let id = self
            .0
            .media_file
            .connect_notify_local(Some("playing"), move |media: &gtk::MediaFile, _| {
                on_playing(media)
            });
        self.0.playing_handlers.borrow_mut().push(id);
    }

    /// Resumes or pauses the current playback.
    pub(crate) fn toggle_pause(&self) {
        let media = &self.0.media_file;
        media.set_playing(!media.is_playing());
    }

    /// Stops the current playback and closes the stream, freeing the
    /// resources of the underlying pipeline, and resets the UI of the
    /// widget that owned it.
    ///
    /// Closing the stream is essential: merely pausing it would leave a
    /// full media pipeline behind, whose teardown happens synchronously
    /// on the main thread when the stream is eventually dropped. If such
    /// a leftover pipeline got stuck, the whole UI would freeze.
    pub(crate) fn stop(&self) {
        if let Some(on_reset_previous) = self.0.on_reset_previous.take() {
            on_reset_previous();
        }

        let old_handlers = std::mem::take(&mut *self.0.playing_handlers.borrow_mut());
        for id in old_handlers {
            self.0.media_file.disconnect(id);
        }

        *self.0.on_prepared.borrow_mut() = None;
        *self.0.on_error.borrow_mut() = None;
        *self.0.current_path.borrow_mut() = None;

        self.0.media_file.pause();
        self.0.media_file.clear();
        if self.0.media_file.is_prepared() {
            self.0.media_file.stream_unprepared();
        }
        self.0.media_file.invalidate_contents();
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::cell::Cell;
    use std::path::PathBuf;
    use std::time::Duration;
    use std::time::Instant;

    use super::*;

    /// Writes a short silent PCM WAV file and returns its path.
    fn silent_wav(name: &str) -> PathBuf {
        let rate: u32 = 8000;
        let samples = vec![0u8; rate as usize / 5]; // 0.2 s, 8 bit mono
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes()); // byte rate
        wav.extend_from_slice(&1u16.to_le_bytes()); // block align
        wav.extend_from_slice(&8u16.to_le_bytes()); // bits per sample
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        wav.extend_from_slice(&samples);

        let path = std::env::temp_dir().join(format!(
            "paper-plane-test-{}-{name}.wav",
            std::process::id()
        ));
        std::fs::write(&path, wav).unwrap();
        path
    }

    fn iterate_until(condition: impl Fn() -> bool) -> bool {
        let context = glib::MainContext::default();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition() {
            if Instant::now() > deadline {
                return false;
            }
            context.iteration(false);
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    }

    /// Hands the shared stream from one playback over to another and stops
    /// it. Needs GTK, see `ui::tests`.
    pub(crate) fn check() {
        eprintln!("playback manager");

        let first = silent_wav("first");
        let second = silent_wav("second");
        let (first_str, second_str) = (first.to_str().unwrap(), second.to_str().unwrap());

        let manager = PlaybackManager::new();
        let resets = Rc::new(Cell::new(0));
        let prepared = Rc::new(Cell::new(0));
        let errors = Rc::new(Cell::new(0));

        let play = |path: &str| {
            manager.play_file(
                path,
                glib::clone!(
                    #[strong]
                    resets,
                    move || resets.set(resets.get() + 1)
                ),
                glib::clone!(
                    #[strong]
                    prepared,
                    move |_| prepared.set(prepared.get() + 1)
                ),
                glib::clone!(
                    #[strong]
                    errors,
                    move || errors.set(errors.get() + 1)
                ),
            );
        };

        play(first_str);
        assert!(manager.is_current(first_str));
        assert!(!manager.is_current(second_str));
        assert!(
            iterate_until(|| prepared.get() > 0 || errors.get() > 0),
            "the stream was neither prepared nor failed"
        );
        assert_eq!(errors.get(), 0, "{:?}", manager.media_file().error());

        // The second playback takes the stream over and resets the first.
        play(second_str);
        assert_eq!(resets.get(), 1);
        assert!(manager.is_current(second_str));
        assert!(iterate_until(|| prepared.get() > 1 || errors.get() > 0));
        assert_eq!(errors.get(), 0, "{:?}", manager.media_file().error());

        manager.toggle_pause();
        assert!(manager.media_file().is_playing());
        manager.toggle_pause();
        assert!(!manager.media_file().is_playing());

        manager.stop();
        assert_eq!(resets.get(), 2);
        assert!(!manager.is_current(second_str));
        assert!(!manager.media_file().is_prepared());

        // Stopping again does not reset the UI twice.
        manager.stop();
        assert_eq!(resets.get(), 2);

        std::fs::remove_file(first).ok();
        std::fs::remove_file(second).ok();
    }
}
