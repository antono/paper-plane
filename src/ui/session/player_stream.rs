//! A `gtk::MediaStream` whose GStreamer pipeline is driven from a worker
//! thread, so that the main thread never waits for it.
//!
//! `gtk::MediaFile` changes the state of its pipeline on the main thread:
//! loading or clearing a file tears the previous pipeline down and joins its
//! thread, which blocks for as long as the audio device takes to respond. On
//! devices whose audio DSP is slow or hangs, the whole UI froze with it.
//!
//! [`PlayerStream`] owns a single long-lived `playbin3` instead. Every state
//! change, seek and query is sent as a [`Command`] to a dedicated thread,
//! and the outcome comes back as an [`Event`] through a channel that is read
//! on the main context. If the pipeline gets stuck, only the worker thread
//! waits: the UI keeps running, and the stream simply doesn't get prepared.
//!
//! Video frames are rendered by `gtk4paintablesink`, whose paintable this
//! stream draws. The sink itself never blocks the main thread: it hands its
//! work over to the main context and waits for it on the streaming threads.
//!
//! Each load gets a new generation number. Bus messages are tagged with the
//! generation that was loaded when they were posted, so that the messages of
//! a previous file are ignored once another one is loaded.

use std::cell::Cell;
use std::cell::RefCell;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::sync::Once;
use std::time::Duration;

use gst::prelude::*;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

/// How often the position is reported while playing.
const POSITION_INTERVAL: Duration = Duration::from_millis(100);

/// A request to the worker thread.
#[derive(Debug)]
enum Command {
    /// Replaces the source of the pipeline and prerolls it.
    Load {
        generation: u64,
        uri: String,
    },
    /// Stops the pipeline, freeing its resources.
    Unload {
        generation: u64,
    },
    Play,
    Pause,
    /// Seeks to the given position, in microseconds.
    Seek(i64),
    Audio {
        muted: bool,
        volume: f64,
    },
    /// Queries the duration and seekability of the prerolled source.
    QueryInfo {
        generation: u64,
    },
    /// Blocks the worker until the sender of the receiver is dropped, like a
    /// pipeline whose audio device doesn't respond.
    #[cfg(test)]
    Stall(mpsc::Receiver<()>),
}

impl Command {
    /// Whether this command replaces the source, making the commands queued
    /// before it irrelevant.
    fn replaces_source(&self) -> bool {
        matches!(self, Self::Load { .. } | Self::Unload { .. })
    }
}

/// A notification from the pipeline or the worker thread.
#[derive(Debug)]
enum Event {
    /// A message posted on the bus while `generation` was loaded.
    Bus {
        generation: u64,
        message: gst::Message,
    },
    /// The answer to [`Command::QueryInfo`]. The duration is in
    /// microseconds, `0` if unknown.
    Info {
        generation: u64,
        duration: i64,
        seekable: bool,
    },
    /// The current position while playing, in microseconds.
    Position { generation: u64, timestamp: i64 },
    /// A seek could not be performed.
    SeekFailed { generation: u64 },
}

/// Initializes GStreamer and registers the statically linked
/// `gtk4paintablesink`.
fn init_gstreamer() {
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        if let Err(error) = gst::init() {
            log::error!("Failed to initialize GStreamer: {error}");
            return;
        }
        if let Err(error) = gstgtk4::plugin_register_static() {
            log::error!("Failed to register gtk4paintablesink: {error}");
        }
    });
}

/// Creates the video sink of the pipeline and the paintable it renders to.
///
/// Must be called on the main thread, which owns the paintable.
fn make_video_sink() -> Option<(gst::Element, gdk::Paintable)> {
    let sink = gst::ElementFactory::make("gtk4paintablesink")
        .build()
        .ok()?;
    let paintable = sink.property::<gdk::Paintable>("paintable");

    // With a GL context, the frames are uploaded by GL elements in front of
    // the sink, otherwise they are converted to a format GTK can show.
    let has_gl = paintable
        .property::<Option<gdk::GLContext>>("gl-context")
        .is_some();
    let element = if has_gl {
        gst::ElementFactory::make("glsinkbin")
            .property("sink", &sink)
            .build()
            .ok()?
    } else {
        let bin = gst::Bin::new();
        let convert = gst::ElementFactory::make("videoconvert").build().ok()?;
        bin.add_many([&convert, &sink]).ok()?;
        convert.link(&sink).ok()?;
        let pad = gst::GhostPad::with_target(&convert.static_pad("sink")?).ok()?;
        bin.add_pad(&pad).ok()?;
        bin.upcast()
    };

    Some((element, paintable))
}

/// Creates the pipeline, preferring `playbin3`.
fn make_pipeline(video_sink: Option<&gst::Element>) -> Option<gst::Element> {
    let pipeline = gst::ElementFactory::make("playbin3")
        .build()
        .or_else(|_| gst::ElementFactory::make("playbin").build())
        .ok()?;

    if let Some(video_sink) = video_sink {
        pipeline.set_property("video-sink", video_sink);
    }

    Some(pipeline)
}

/// Runs the commands sent to the pipeline until the stream is dropped.
fn run_worker(
    pipeline: gst::Element,
    commands: mpsc::Receiver<Command>,
    events: async_channel::Sender<Event>,
    loaded: Arc<AtomicU64>,
) {
    let mut queue: Vec<Command> = Vec::new();
    let mut playing = false;

    loop {
        // Wait for a command, reporting the position meanwhile.
        if queue.is_empty() {
            let command = if playing {
                match commands.recv_timeout(POSITION_INTERVAL) {
                    Ok(command) => command,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if let Some(position) = pipeline.query_position::<gst::ClockTime>() {
                            let _ = events.send_blocking(Event::Position {
                                generation: loaded.load(Ordering::Acquire),
                                timestamp: position.useconds() as i64,
                            });
                        }
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match commands.recv() {
                    Ok(command) => command,
                    Err(_) => break,
                }
            };
            queue.push(command);
        }

        // Commands that piled up while the pipeline was busy are applied at
        // once, skipping those that a later change of source supersedes.
        queue.extend(commands.try_iter());
        if let Some(last) = queue.iter().rposition(Command::replaces_source) {
            queue.drain(..last);
        }

        for command in std::mem::take(&mut queue) {
            match command {
                Command::Load { generation, uri } => {
                    let _ = pipeline.set_state(gst::State::Null);
                    playing = false;
                    // Nothing is streaming in the NULL state, so every
                    // message posted from now on belongs to the new source.
                    loaded.store(generation, Ordering::Release);
                    pipeline.set_property("uri", &uri);
                    if pipeline.set_state(gst::State::Paused).is_err() {
                        log::warn!("Failed to preroll {uri}");
                    }
                }
                Command::Unload { generation } => {
                    let _ = pipeline.set_state(gst::State::Null);
                    playing = false;
                    loaded.store(generation, Ordering::Release);
                }
                Command::Play => {
                    let _ = pipeline.set_state(gst::State::Playing);
                    playing = true;
                }
                Command::Pause => {
                    let _ = pipeline.set_state(gst::State::Paused);
                    playing = false;
                }
                Command::Seek(timestamp) => {
                    let position = gst::ClockTime::from_useconds(timestamp.max(0) as u64);
                    if pipeline
                        .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE, position)
                        .is_err()
                    {
                        let _ = events.send_blocking(Event::SeekFailed {
                            generation: loaded.load(Ordering::Acquire),
                        });
                    }
                }
                Command::Audio { muted, volume } => {
                    pipeline.set_property("mute", muted);
                    pipeline.set_property("volume", volume);
                }
                #[cfg(test)]
                Command::Stall(release) => {
                    let _ = release.recv();
                }
                Command::QueryInfo { generation } => {
                    let duration = pipeline
                        .query_duration::<gst::ClockTime>()
                        .map(|duration| duration.useconds() as i64)
                        .unwrap_or(0);
                    let mut query = gst::query::Seeking::new(gst::Format::Time);
                    let seekable = pipeline.query(&mut query) && query.result().0;
                    let _ = events.send_blocking(Event::Info {
                        generation,
                        duration,
                        seekable,
                    });
                }
            }
        }
    }

    let _ = pipeline.set_state(gst::State::Null);
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct PlayerStream {
        /// The sender of the commands to the worker thread. Created with the
        /// pipeline on the first load.
        pub(super) commands: RefCell<Option<mpsc::Sender<Command>>>,
        /// The paintable the video sink renders to.
        pub(super) paintable: RefCell<Option<gdk::Paintable>>,
        /// The generation of the most recently requested source.
        pub(super) generation: Cell<u64>,
        /// Whether the info of the prerolled source has been requested.
        pub(super) info_requested: Cell<bool>,
        /// Whether the selected streams of the source contain audio and
        /// video, as reported by the pipeline.
        pub(super) has_audio: Cell<Option<bool>>,
        pub(super) has_video: Cell<Option<bool>>,
        /// The position a seek is pending to, in microseconds.
        pub(super) pending_seek: Cell<Option<i64>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PlayerStream {
        const NAME: &'static str = "PaplPlayerStream";
        type Type = super::PlayerStream;
        type ParentType = gtk::MediaStream;
        type Interfaces = (gdk::Paintable,);
    }

    impl ObjectImpl for PlayerStream {
        fn dispose(&self) {
            // The worker stops the pipeline and exits once the sender is
            // gone. It is not waited for.
            self.commands.take();
        }
    }

    impl PaintableImpl for PlayerStream {
        fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
            // Nothing is drawn until the source is prepared, so that the
            // last frame of the previous one is never shown.
            if let Some(paintable) = self.visible_paintable() {
                paintable.snapshot(snapshot, width, height);
            }
        }

        fn current_image(&self) -> gdk::Paintable {
            match self.visible_paintable() {
                Some(paintable) => paintable.current_image(),
                None => gdk::Paintable::new_empty(0, 0),
            }
        }

        fn intrinsic_width(&self) -> i32 {
            self.visible_paintable()
                .map_or(0, |paintable| paintable.intrinsic_width())
        }

        fn intrinsic_height(&self) -> i32 {
            self.visible_paintable()
                .map_or(0, |paintable| paintable.intrinsic_height())
        }

        fn intrinsic_aspect_ratio(&self) -> f64 {
            self.visible_paintable()
                .map_or(0.0, |paintable| paintable.intrinsic_aspect_ratio())
        }
    }

    impl MediaStreamImpl for PlayerStream {
        fn play(&self) -> bool {
            let obj = self.obj();

            if obj.is_ended() {
                self.send(Command::Seek(0));
            }
            self.send(Command::Play)
        }

        fn pause(&self) {
            self.send(Command::Pause);
        }

        fn seek(&self, timestamp: i64) {
            self.pending_seek.set(Some(timestamp));
            if !self.send(Command::Seek(timestamp)) {
                self.pending_seek.set(None);
                self.obj().seek_failed();
            }
        }

        fn update_audio(&self, muted: bool, volume: f64) {
            self.send(Command::Audio { muted, volume });
        }

        fn realize(&self, _surface: gdk::Surface) {}

        fn unrealize(&self, _surface: gdk::Surface) {}
    }

    impl PlayerStream {
        /// Sends a command to the worker thread. Returns whether there is
        /// one to send it to.
        pub(super) fn send(&self, command: Command) -> bool {
            match &*self.commands.borrow() {
                Some(commands) => commands.send(command).is_ok(),
                None => false,
            }
        }

        /// The paintable of the video sink, if the current source is
        /// prepared.
        fn visible_paintable(&self) -> Option<gdk::Paintable> {
            let obj = self.obj();
            if !obj.is_prepared() || obj.error().is_some() {
                return None;
            }
            self.paintable.borrow().clone()
        }

        /// Creates the pipeline and the worker thread, if not done yet.
        pub(super) fn ensure_pipeline(&self) -> bool {
            if self.commands.borrow().is_some() {
                return true;
            }

            init_gstreamer();

            let video_sink = make_video_sink();
            if video_sink.is_none() {
                log::warn!("No video sink, only audio can be played");
            }
            let Some(pipeline) = make_pipeline(video_sink.as_ref().map(|(sink, _)| sink)) else {
                log::error!("Failed to create the playback pipeline");
                return false;
            };

            if let Some((_, paintable)) = video_sink {
                let obj = self.obj();
                paintable.connect_invalidate_contents(glib::clone!(
                    #[weak]
                    obj,
                    move |_| {
                        if obj.is_prepared() {
                            obj.invalidate_contents();
                        }
                    }
                ));
                paintable.connect_invalidate_size(glib::clone!(
                    #[weak]
                    obj,
                    move |_| {
                        if obj.is_prepared() {
                            obj.invalidate_size();
                        }
                    }
                ));
                self.paintable.replace(Some(paintable));
            }

            let (event_sender, event_receiver) = async_channel::unbounded();
            let loaded = Arc::new(AtomicU64::new(0));

            // Bus messages are forwarded from the thread that posts them, so
            // that they are tagged with the generation they belong to and
            // arrive in order with the events of the worker.
            let bus = pipeline.bus().unwrap();
            bus.set_sync_handler(glib::clone!(
                #[strong]
                event_sender,
                #[strong]
                loaded,
                move |_, message| {
                    use gst::MessageView;

                    let relevant = match message.view() {
                        MessageView::Eos(_)
                        | MessageView::Error(_)
                        | MessageView::StreamsSelected(_) => true,
                        MessageView::AsyncDone(_) => {
                            message.src().is_some_and(|src| src.parent().is_none())
                        }
                        _ => false,
                    };
                    if relevant {
                        let _ = event_sender.try_send(Event::Bus {
                            generation: loaded.load(Ordering::Acquire),
                            message: message.clone(),
                        });
                    }
                    gst::BusSyncReply::Drop
                }
            ));

            let (command_sender, command_receiver) = mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("paper-plane-player".into())
                .spawn(move || run_worker(pipeline, command_receiver, event_sender, loaded));
            if let Err(error) = spawned {
                log::error!("Failed to start the playback thread: {error}");
                return false;
            }
            self.commands.replace(Some(command_sender));

            let obj = self.obj().downgrade();
            glib::spawn_future_local(async move {
                while let Ok(event) = event_receiver.recv().await {
                    let Some(obj) = obj.upgrade() else {
                        break;
                    };
                    obj.imp().handle_event(event);
                }
            });

            true
        }

        fn handle_event(&self, event: Event) {
            let obj = self.obj();
            let current = self.generation.get();

            match event {
                Event::Bus {
                    generation,
                    message,
                } if generation == current => self.handle_message(&message),
                Event::Info {
                    generation,
                    duration,
                    seekable,
                } if generation == current => {
                    if obj.is_prepared() || obj.error().is_some() {
                        return;
                    }
                    obj.stream_prepared(
                        self.has_audio.get().unwrap_or(true),
                        self.has_video.get().unwrap_or(true),
                        seekable,
                        duration,
                    );
                    obj.invalidate_size();
                    obj.invalidate_contents();
                }
                Event::Position {
                    generation,
                    timestamp,
                } if generation == current => {
                    if obj.is_prepared() && self.pending_seek.get().is_none() {
                        obj.update(timestamp);
                    }
                }
                Event::SeekFailed { generation }
                    if generation == current && self.pending_seek.take().is_some() =>
                {
                    obj.seek_failed();
                }
                _ => {}
            }
        }

        fn handle_message(&self, message: &gst::Message) {
            use gst::MessageView;

            let obj = self.obj();

            match message.view() {
                MessageView::AsyncDone(_) => {
                    if let Some(timestamp) = self.pending_seek.take() {
                        obj.seek_success();
                        obj.update(timestamp);
                    } else if !obj.is_prepared() && !self.info_requested.replace(true) {
                        self.send(Command::QueryInfo {
                            generation: self.generation.get(),
                        });
                    }
                }
                MessageView::StreamsSelected(selected) => {
                    let types = selected
                        .streams()
                        .fold(gst::StreamType::empty(), |types, stream| {
                            types | stream.stream_type()
                        });
                    self.has_audio
                        .set(Some(types.contains(gst::StreamType::AUDIO)));
                    self.has_video
                        .set(Some(types.contains(gst::StreamType::VIDEO)));
                }
                MessageView::Eos(_) => {
                    if obj.is_loop() {
                        self.send(Command::Seek(0));
                    } else {
                        self.send(Command::Pause);
                        if obj.is_prepared() && !obj.is_ended() {
                            obj.update(obj.duration());
                            obj.stream_ended();
                        }
                    }
                }
                MessageView::Error(error) => {
                    log::warn!("Playback failed: {} ({:?})", error.error(), error.debug());
                    self.send(Command::Unload {
                        generation: self.generation.get(),
                    });
                    if obj.error().is_none() {
                        obj.set_error(error.error());
                    }
                }
                _ => {}
            }
        }
    }
}

glib::wrapper! {
    /// A media stream whose pipeline never blocks the main thread. See the
    /// module docs.
    pub(crate) struct PlayerStream(ObjectSubclass<imp::PlayerStream>)
        @extends gtk::MediaStream,
        @implements gdk::Paintable;
}

impl Default for PlayerStream {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl PlayerStream {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Loads the file at `path` and prerolls it, replacing the current
    /// source. The stream is prepared once the first frame is available.
    pub(crate) fn load(&self, path: &str) {
        let imp = self.imp();

        self.reset();

        if !imp.ensure_pipeline() {
            self.set_error(glib::Error::new(
                gtk::gio::IOErrorEnum::Failed,
                "No playback pipeline",
            ));
            return;
        }

        let uri = match glib::filename_to_uri(path, None) {
            Ok(uri) => uri.to_string(),
            Err(error) => {
                self.set_error(error);
                return;
            }
        };

        imp.send(Command::Audio {
            muted: self.is_muted(),
            volume: self.volume(),
        });
        imp.send(Command::Load {
            generation: imp.generation.get(),
            uri,
        });
    }

    /// Stops the pipeline and unloads the current source.
    pub(crate) fn unload(&self) {
        let imp = self.imp();

        self.reset();

        imp.send(Command::Unload {
            generation: imp.generation.get(),
        });
    }

    /// Resets the state of the stream for a new generation of the source.
    fn reset(&self) {
        let imp = self.imp();

        imp.generation.set(imp.generation.get() + 1);
        imp.info_requested.set(false);
        imp.has_audio.set(None);
        imp.has_video.set(None);

        if imp.pending_seek.take().is_some() {
            self.seek_failed();
        }
        if self.is_playing() {
            self.pause();
        }
        if self.is_prepared() {
            self.stream_unprepared();
        }
        self.invalidate_size();
        self.invalidate_contents();
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::path::Path;
    use std::rc::Rc;
    use std::time::Instant;

    use super::*;
    use crate::ui::session::playback_manager::tests::silent_wav;

    /// Runs the main context until `condition` holds, for at most 10 s.
    fn iterate_until(condition: impl Fn() -> bool) -> bool {
        let context = glib::MainContext::default();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition() {
            if Instant::now() > deadline {
                return false;
            }
            context.iteration(false);
            std::thread::sleep(Duration::from_millis(2));
        }
        true
    }

    /// Runs the main context for `duration`.
    fn iterate_for(duration: Duration) {
        let deadline = Instant::now() + duration;
        iterate_until(|| Instant::now() > deadline);
    }

    fn load_prepared(stream: &PlayerStream, path: &Path) {
        stream.load(path.to_str().unwrap());
        assert!(
            iterate_until(|| stream.is_prepared()),
            "{} was not prepared",
            path.display()
        );
        assert_eq!(stream.error(), None);
    }

    /// The playback of a file from preroll to its end, seeks, errors and the
    /// main thread staying responsive while the pipeline is stuck. Needs
    /// GTK, see `ui::tests`.
    pub(crate) fn check() {
        eprintln!("player stream");

        let wav = silent_wav("player-stream");
        let stream = PlayerStream::new();
        let ended = Rc::new(Cell::new(0));
        stream.connect_ended_notify(glib::clone!(
            #[strong]
            ended,
            move |stream| {
                if stream.is_ended() {
                    ended.set(ended.get() + 1);
                }
            }
        ));

        // Prepared with the info of the source, without starting it.
        load_prepared(&stream, &wav);
        assert!(stream.has_audio());
        assert!(!stream.has_video());
        assert!(stream.is_seekable());
        assert!(
            (150_000..=250_000).contains(&stream.duration()),
            "duration {}",
            stream.duration()
        );
        assert!(!stream.is_playing());

        // Played to the end, and again from the start.
        stream.play();
        assert!(stream.is_playing());
        assert!(iterate_until(|| ended.get() == 1), "the stream did not end");
        assert!(!stream.is_playing());
        stream.play();
        assert!(!stream.is_ended());
        assert!(iterate_until(|| ended.get() == 2), "the replay did not end");

        // A seek is completed with the position it went to.
        stream.seek(100_000);
        assert!(stream.is_seeking());
        assert!(iterate_until(|| !stream.is_seeking()), "the seek hung");
        assert_eq!(stream.timestamp(), 100_000);

        // A looping stream starts over instead of ending.
        stream.set_loop(true);
        stream.play();
        iterate_for(Duration::from_millis(600));
        assert_eq!(ended.get(), 2);
        assert!(stream.is_playing());
        stream.set_loop(false);
        stream.pause();

        // Unloading resets the stream, and nothing of the unloaded source
        // arrives later.
        stream.unload();
        assert!(!stream.is_prepared());
        iterate_for(Duration::from_millis(200));
        assert!(!stream.is_prepared());
        assert_eq!(stream.error(), None);

        // A missing file fails; the next load clears the error.
        stream.load("/nonexistent/paper-plane-test.wav");
        assert!(
            iterate_until(|| stream.error().is_some()),
            "a missing file did not fail"
        );
        load_prepared(&stream, &wav);

        // A load that is superseded right away never prepares the stream.
        stream.load("/nonexistent/paper-plane-test.wav");
        load_prepared(&stream, &wav);
        iterate_for(Duration::from_millis(200));
        assert_eq!(stream.error(), None);

        // While the pipeline is stuck, nothing waits for it: the calls
        // return right away and the main context keeps running. Once it
        // recovers, only the last load is applied.
        let (release, stalled) = mpsc::channel();
        assert!(stream.imp().send(Command::Stall(stalled)));
        let started = Instant::now();
        stream.play();
        stream.pause();
        stream.unload();
        stream.load(wav.to_str().unwrap());
        stream.unload();
        stream.load(wav.to_str().unwrap());
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "the calls waited for the pipeline: {:?}",
            started.elapsed()
        );
        let ticks = Rc::new(Cell::new(0));
        let timer = glib::timeout_add_local(
            Duration::from_millis(10),
            glib::clone!(
                #[strong]
                ticks,
                move || {
                    ticks.set(ticks.get() + 1);
                    glib::ControlFlow::Continue
                }
            ),
        );
        iterate_for(Duration::from_millis(300));
        timer.remove();
        assert!(ticks.get() >= 10, "the main context stalled");
        assert!(!stream.is_prepared());
        drop(release);
        assert!(
            iterate_until(|| stream.is_prepared()),
            "the last load was not applied after the pipeline recovered"
        );
        assert_eq!(stream.error(), None);

        std::fs::remove_file(wav).ok();
    }
}
