use std::cell::Cell;
use std::cell::RefCell;

use glib::clone;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::super::playback_manager;
use super::media_viewer::ViewerItem;
use super::message_row::FileStatus;
use super::message_row::MediaDownloadButton;
use super::message_row::MediaLoader;
use crate::model;
use crate::model::MediaType;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct MediaViewerPage {
        pub(super) overlay: gtk::Overlay,
        /// Scrolls the photo while it is zoomed in.
        pub(super) zoom_scroll: gtk::ScrolledWindow,
        pub(super) picture: gtk::Picture,
        /// The zoom of the photo: 1 fits it into the page.
        pub(super) zoom: Cell<f64>,
        /// The zoom when a pinch started.
        pub(super) zoom_start: Cell<f64>,
        /// The point of the unzoomed photo under the fingers when a pinch
        /// started: it stays under them while they move.
        pub(super) pinch_point: Cell<Option<(f64, f64)>>,
        /// Where to scroll the zoomed photo once it is allocated, so that
        /// the point under the pointer or the fingers stays in place.
        pub(super) pending_scroll: Cell<Option<(f64, f64)>>,
        /// The last position of the pointer over the page.
        pub(super) pointer: Cell<Option<(f64, f64)>>,
        /// The scroll position when a mouse drag started.
        pub(super) drag_start: Cell<(f64, f64)>,
        /// The video widget rendering the media stream of a video item, with
        /// graphics offload enabled and its own built-in media controls.
        /// Only shown while a stream is bound to it.
        pub(super) video: gtk::Video,
        pub(super) download_button: MediaDownloadButton,
        pub(super) click: gtk::GestureClick,
        pub(super) loader: MediaLoader,
        /// The item shown by this page.
        pub(super) item: RefCell<Option<ViewerItem>>,
        /// The shared playback engine used to play the video of this page.
        pub(super) playback: RefCell<Option<playback_manager::PlaybackManager>>,
        /// The low-resolution preview shown until the media is displayed.
        pub(super) placeholder: RefCell<Option<gdk::Texture>>,
        /// Whether this page is the one displayed by the viewer.
        pub(super) displayed: Cell<bool>,
        /// Whether the photo of this page has been decoded already.
        pub(super) photo_decoded: Cell<bool>,
        /// Whether the picture shows the media stream of the playback
        /// engine, in which case it must be restored to the preview when
        /// the playback is taken over or stopped.
        pub(super) is_stream_bound: Cell<bool>,
        /// Incremented every time new media is set, used to discard the
        /// results of asynchronous operations that became out-of-date.
        pub(super) generation: Cell<u64>,
        pub(super) loader_handler_id: RefCell<Option<glib::SignalHandlerId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MediaViewerPage {
        const NAME: &'static str = "PaplMediaViewerPage";
        type Type = super::MediaViewerPage;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for MediaViewerPage {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: std::sync::OnceLock<Vec<glib::ParamSpec>> =
                std::sync::OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![glib::ParamSpecDouble::builder("zoom")
                    .minimum(1.0)
                    .maximum(MAX_ZOOM)
                    .default_value(1.0)
                    .read_only()
                    .build()]
            })
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "zoom" => self.zoom.get().max(1.0).to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            obj.set_hexpand(true);
            obj.set_vexpand(true);

            self.zoom.set(1.0);
            self.picture.set_hexpand(true);
            self.picture.set_vexpand(true);
            self.zoom_scroll
                .set_policy(gtk::PolicyType::External, gtk::PolicyType::External);
            self.zoom_scroll.set_child(Some(&self.picture));
            if let Some(viewport) = self.zoom_scroll.child().and_downcast::<gtk::Viewport>() {
                viewport.set_scroll_to_focus(false);
            }
            self.overlay.set_child(Some(&self.zoom_scroll));
            obj.setup_zoom_gestures();

            self.video.set_hexpand(true);
            self.video.set_vexpand(true);
            self.video
                .set_graphics_offload(gtk::GraphicsOffloadEnabled::Enabled);
            self.video.set_visible(false);
            self.overlay.add_overlay(&self.video);

            self.download_button.set_halign(gtk::Align::Center);
            self.download_button.set_valign(gtk::Align::Center);
            self.download_button.set_visible(false);
            self.overlay.add_overlay(&self.download_button);

            self.click.set_button(1);
            self.overlay.add_controller(self.click.clone());

            self.overlay.set_parent(&*obj);

            let handler_id = self.loader.connect_status_notify(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.update_status();
                }
            ));
            *self.loader_handler_id.borrow_mut() = Some(handler_id);

            self.click.connect_released(clone!(
                #[weak]
                obj,
                move |_, n_press, x, y| {
                    // A double click or tap zooms the photo in, or back out.
                    if n_press == 2 && obj.can_zoom() {
                        let zoom = if obj.zoom() > 1.0 {
                            1.0
                        } else {
                            DOUBLE_TAP_ZOOM
                        };
                        obj.set_zoom(zoom, Some((x, y)));
                        return;
                    }
                    obj.handle_click();
                }
            ));
        }

        fn dispose(&self) {
            if let Some(handler_id) = self.loader_handler_id.borrow_mut().take() {
                self.loader.disconnect(handler_id);
            }

            self.overlay.unparent();
        }
    }

    impl WidgetImpl for MediaViewerPage {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            self.overlay.measure(orientation, for_size)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            // Follow the page size, e.g. when the window is resized.
            let zoom = self.zoom.get();
            if zoom > 1.0 {
                let size = (
                    (width as f64 * zoom).round() as i32,
                    (height as f64 * zoom).round() as i32,
                );
                if self.picture.size_request() != size {
                    self.picture.set_size_request(size.0, size.1);
                }
            }

            self.overlay.allocate(width, height, baseline, None);

            if let Some((x, y)) = self.pending_scroll.take() {
                self.zoom_scroll.hadjustment().set_value(x);
                self.zoom_scroll.vadjustment().set_value(y);
            }
        }
    }
}

glib::wrapper! {
    pub(crate) struct MediaViewerPage(ObjectSubclass<imp::MediaViewerPage>)
        @extends gtk::Widget;
}

/// The zoom of a double click or tap.
const DOUBLE_TAP_ZOOM: f64 = 2.5;
/// The largest zoom of a photo.
pub(crate) const MAX_ZOOM: f64 = 8.0;
/// The zoom step of the buttons, the keys and Ctrl+scrolling.
pub(crate) const ZOOM_STEP: f64 = 1.25;

impl Default for MediaViewerPage {
    fn default() -> Self {
        glib::Object::new()
    }
}

/// A page of the media viewer, showing a single photo or video.
///
/// The page is self-contained: it shows the low-resolution preview of its
/// item until it is displayed, decodes its photo or plays its video on the
/// shared playback stream of the session, which its `gtk::Video` provides
/// the media controls for. Videos are never played on a pipeline of their
/// own: the playback engine of the session owns the single media stream,
/// which the page binds to its graphics-offloaded `gtk::Video` while it is
/// displayed.
impl MediaViewerPage {
    /// Shows the given item.
    pub(crate) fn set_item(
        &self,
        session: &model::ClientStateSession,
        item: ViewerItem,
        playback: Option<&playback_manager::PlaybackManager>,
    ) {
        let imp = self.imp();

        imp.generation.set(imp.generation.get() + 1);
        imp.displayed.set(false);
        self.set_zoom(1.0, None);
        imp.photo_decoded.set(false);
        imp.is_stream_bound.set(false);

        *imp.playback.borrow_mut() = playback.cloned();

        let placeholder = item.placeholder().cloned();
        imp.picture.set_paintable(placeholder.as_ref());
        *imp.placeholder.borrow_mut() = placeholder;

        let media_type = match &item {
            ViewerItem::Photo { .. } => MediaType::Photo,
            ViewerItem::Video { .. } => MediaType::Video,
        };
        imp.loader.bind(session, media_type, item.file());

        *imp.item.borrow_mut() = Some(item);

        imp.video.set_media_stream(gtk::MediaStream::NONE);
        imp.video.set_visible(false);

        self.update_status();
    }

    /// The path of the media file of this item, if it has been downloaded.
    fn media_path(&self) -> Option<glib::GString> {
        if self.imp().loader.status() != FileStatus::Downloaded {
            return None;
        }

        self.imp().loader.path()
    }

    /// Displays the media of this page.
    ///
    /// This is called by the viewer when this page becomes the active one.
    /// The download of the media is started here as well, so that the media
    /// of the pages that are only swiped past is not downloaded.
    pub(crate) fn activate(&self) {
        let imp = self.imp();

        imp.displayed.set(true);

        imp.loader.maybe_start_auto_download();

        match &*imp.item.borrow() {
            Some(ViewerItem::Photo { .. }) => {
                imp.picture.set_paintable(imp.placeholder.borrow().as_ref());

                self.maybe_decode_photo();
            }
            Some(ViewerItem::Video { .. }) => {
                if imp.loader.status() == FileStatus::CanBeDownloaded {
                    imp.loader.request_download();
                }

                if self.media_path().is_some() {
                    self.start_playback()
                }
            }
            None => {}
        }
    }

    /// Stops displaying the media of this page.
    ///
    /// The decoded photo or the video frame bound by the playback is
    /// dropped, so that at most one item is fully decoded at any time, and
    /// the playback of the video is stopped if this page owns the shared
    /// media stream.
    pub(crate) fn deactivate(&self) {
        let imp = self.imp();

        imp.displayed.set(false);
        self.set_zoom(1.0, None);

        if let (Some(manager), Some(path)) = (imp.playback.borrow().clone(), self.media_path()) {
            if manager.is_current(&path) {
                manager.stop();
            }
        }

        self.reset_playback_ui();

        imp.picture.set_paintable(imp.placeholder.borrow().as_ref());
    }

    /// Unbinds the media stream and hides the video widget of this page.
    fn reset_playback_ui(&self) {
        let imp = self.imp();

        if imp.is_stream_bound.get() {
            imp.is_stream_bound.set(false);
            imp.video.set_media_stream(gtk::MediaStream::NONE);
            imp.video.set_visible(false);
            imp.picture.set_paintable(imp.placeholder.borrow().as_ref());
            imp.picture.set_visible(true);
        }
    }

    /// Plays the video of this page on the shared media stream of the
    /// session.
    ///
    /// Animations loop and stay muted, while videos are played with sound.
    fn start_playback(&self) {
        let imp = self.imp();

        let is_animation = match &*imp.item.borrow() {
            Some(ViewerItem::Video { is_animation, .. }) => *is_animation,
            _ => return,
        };
        let Some(manager) = imp.playback.borrow().clone() else {
            return;
        };
        let Some(path) = self.media_path() else {
            return;
        };

        imp.is_stream_bound.set(false);
        imp.picture.set_paintable(imp.placeholder.borrow().as_ref());

        let generation = imp.generation.get();

        manager.play_file(
            &path,
            clone!(
                #[weak(rename_to = page)]
                self,
                move || {
                    page.reset_playback_ui();
                }
            ),
            clone!(
                #[weak(rename_to = page)]
                self,
                move |media: &gtk::MediaFile| {
                    let imp = page.imp();

                    if !imp.displayed.get() || imp.generation.get() != generation {
                        return;
                    }

                    imp.is_stream_bound.set(true);
                    imp.picture.set_paintable(gdk::Paintable::NONE);
                    imp.picture.set_visible(false);
                    imp.video.set_media_stream(Some(media));
                    imp.video.set_visible(true);

                    // Set on the prepared stream: before, a stream closed by
                    // a previous stop() has no player to apply them to.
                    media.set_muted(is_animation);
                    media.set_loop(is_animation);
                    media.play();
                }
            ),
            clone!(
                #[weak(rename_to = page)]
                self,
                move || {
                    page.reset_playback_ui();
                }
            ),
        );
    }

    /// Updates the widgets according to the status of the loader.
    fn update_status(&self) {
        let imp = self.imp();

        match imp.loader.status() {
            FileStatus::Downloaded => {
                imp.download_button.set_visible(false);

                match &*imp.item.borrow() {
                    Some(ViewerItem::Photo { .. }) => {
                        self.maybe_decode_photo();
                        // It can be zoomed now.
                        self.notify("zoom");
                    }
                    Some(ViewerItem::Video { .. }) if imp.displayed.get() => self.start_playback(),
                    Some(ViewerItem::Video { .. }) => {}
                    None => {}
                }
            }
            FileStatus::Downloading(progress) => {
                imp.download_button.set_visible(true);
                imp.download_button
                    .set_status(FileStatus::Downloading(progress));
            }
            FileStatus::CanBeDownloaded => {
                imp.download_button.set_visible(true);
                imp.download_button.set_status(FileStatus::CanBeDownloaded);
            }
            FileStatus::Uploading(_) => {}
        }
    }

    /// Decodes the photo of this page on a worker thread, if it has been
    /// downloaded and not decoded yet.
    ///
    /// Pages that are not displayed are decoded too, so that the whole
    /// carousel shows the full images of the media that is already
    /// downloaded instead of the low-resolution previews. The result of a
    /// page that stopped being displayed in the meantime is stored as its
    /// preview rather than bound to its picture.
    fn maybe_decode_photo(&self) {
        let imp = self.imp();

        if imp.photo_decoded.get() {
            return;
        }

        let Some(path) = imp.loader.path() else {
            return;
        };

        imp.photo_decoded.set(true);

        let generation = imp.generation.get();
        let path = path.to_string();

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let result = gio::spawn_blocking(move || utils::decode_image_from_path(&path))
                    .await
                    .unwrap();

                if obj.imp().generation.get() != generation {
                    return;
                }

                match result {
                    Ok(texture) => {
                        let imp = obj.imp();

                        *imp.placeholder.borrow_mut() = Some(texture.clone().into());

                        if imp.displayed.get() {
                            imp.picture.set_paintable(Some(&texture));
                        }
                    }
                    Err(e) => {
                        log::warn!("Error decoding a photo: {e:?}");
                    }
                }
            }
        ));
    }

    /// The zoom of the photo: 1 fits it into the page.
    pub(crate) fn zoom(&self) -> f64 {
        self.imp().zoom.get().max(1.0)
    }

    /// Whether the item can be zoomed: a downloaded photo.
    pub(crate) fn can_zoom(&self) -> bool {
        let imp = self.imp();
        matches!(&*imp.item.borrow(), Some(ViewerItem::Photo { .. }))
            && imp.loader.status() == FileStatus::Downloaded
    }

    /// Zooms the photo to `zoom` (clamped to 1..=MAX_ZOOM), keeping the
    /// point at `anchor` (in page coordinates, the center by default) in
    /// place.
    pub(crate) fn set_zoom(&self, zoom: f64, anchor: Option<(f64, f64)>) {
        let zoom = if self.can_zoom() {
            zoom.clamp(1.0, MAX_ZOOM)
        } else {
            1.0
        };
        let old = self.zoom();
        if (zoom - old).abs() < f64::EPSILON {
            return;
        }

        let anchor = anchor.unwrap_or((self.width() as f64 / 2.0, self.height() as f64 / 2.0));
        let point = self.photo_point_at(anchor);
        self.zoom_keeping(zoom, point, anchor);
    }

    /// The scroll position of the zoomed photo, including a scroll that is
    /// still to be applied: several zoom steps can come before the next
    /// allocation, e.g. while pinching.
    fn scroll_position(&self) -> (f64, f64) {
        let imp = self.imp();
        imp.pending_scroll.get().unwrap_or_else(|| {
            (
                imp.zoom_scroll.hadjustment().value(),
                imp.zoom_scroll.vadjustment().value(),
            )
        })
    }

    /// The point of the unzoomed photo at `(x, y)` in page coordinates.
    fn photo_point_at(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let zoom = self.zoom();
        let (sx, sy) = self.scroll_position();
        ((sx + x) / zoom, (sy + y) / zoom)
    }

    /// Zooms to `zoom` (already clamped) and scrolls so that `point` of the
    /// unzoomed photo is at `anchor` in page coordinates.
    fn zoom_keeping(&self, zoom: f64, (px, py): (f64, f64), (x, y): (f64, f64)) {
        let imp = self.imp();
        let (width, height) = (self.width() as f64, self.height() as f64);

        let changed = (zoom - self.zoom()).abs() >= f64::EPSILON;
        imp.zoom.set(zoom);
        if zoom > 1.0 {
            imp.picture.set_size_request(
                (width * zoom).round() as i32,
                (height * zoom).round() as i32,
            );
            imp.pending_scroll.set(Some((px * zoom - x, py * zoom - y)));
        } else {
            imp.picture.set_size_request(-1, -1);
            imp.pending_scroll.set(None);
        }
        self.queue_resize();
        if changed {
            self.notify("zoom");
        }
    }

    /// Starts a pinch with the fingers around `center`, if the item can be
    /// zoomed.
    fn begin_pinch(&self, center: (f64, f64)) -> bool {
        let imp = self.imp();
        if !self.can_zoom() {
            return false;
        }
        imp.zoom_start.set(self.zoom());
        imp.pinch_point.set(Some(self.photo_point_at(center)));
        true
    }

    /// Follows the fingers of a pinch: the photo point that was under them
    /// when it started stays under them, also when they move together.
    fn update_pinch(&self, scale: f64, center: (f64, f64)) {
        let imp = self.imp();
        let Some(point) = imp.pinch_point.get() else {
            return;
        };
        if !self.can_zoom() {
            return;
        }
        let zoom = (imp.zoom_start.get() * scale).clamp(1.0, MAX_ZOOM);
        self.zoom_keeping(zoom, point, center);
    }

    /// Multiplies the zoom by `factor` around `anchor`, or the pointer.
    pub(crate) fn zoom_by(&self, factor: f64, anchor: Option<(f64, f64)>) {
        let anchor = anchor.or(self.imp().pointer.get());
        self.set_zoom(self.zoom() * factor, anchor);
    }

    /// Pinching, Ctrl+scrolling and dragging the zoomed photo with the
    /// mouse (touch drags are scrolled by the scrolled window).
    fn setup_zoom_gestures(&self) {
        let pinch = gtk::GestureZoom::new();
        pinch.set_propagation_phase(gtk::PropagationPhase::Capture);
        pinch.connect_begin(clone!(
            #[weak(rename_to = obj)]
            self,
            move |gesture, _| {
                if gesture
                    .bounding_box_center()
                    .is_some_and(|center| obj.begin_pinch(center))
                {
                    // The fingers zoom and move the photo now: the scrolled
                    // window must not scroll it too, nor the carousel swipe.
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                }
            }
        ));
        pinch.connect_scale_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |gesture, scale| {
                if let Some(center) = gesture.bounding_box_center() {
                    obj.update_pinch(scale, center);
                }
            }
        ));
        pinch.connect_end(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, _| obj.imp().pinch_point.set(None)
        ));
        self.add_controller(pinch);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        scroll.connect_scroll(clone!(
            #[weak(rename_to = obj)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |controller, _, dy| {
                if !controller
                    .current_event_state()
                    .contains(gdk::ModifierType::CONTROL_MASK)
                    || !obj.can_zoom()
                {
                    return glib::Propagation::Proceed;
                }
                let factor = if dy < 0.0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
                obj.zoom_by(factor, None);
                glib::Propagation::Stop
            }
        ));
        self.add_controller(scroll);

        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, x, y| obj.imp().pointer.set(Some((x, y)))
        ));
        motion.connect_leave(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.imp().pointer.set(None)
        ));
        self.add_controller(motion);

        let drag = gtk::GestureDrag::new();
        drag.connect_drag_begin(clone!(
            #[weak(rename_to = obj)]
            self,
            move |gesture, _, _| {
                let is_touch = gesture
                    .device()
                    .is_some_and(|d| d.source() == gdk::InputSource::Touchscreen);
                if obj.zoom() <= 1.0 || is_touch {
                    gesture.set_state(gtk::EventSequenceState::Denied);
                    return;
                }
                let imp = obj.imp();
                imp.drag_start.set((
                    imp.zoom_scroll.hadjustment().value(),
                    imp.zoom_scroll.vadjustment().value(),
                ));
            }
        ));
        drag.connect_drag_update(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, dx, dy| {
                let imp = obj.imp();
                let (x, y) = imp.drag_start.get();
                imp.zoom_scroll.hadjustment().set_value(x - dx);
                imp.zoom_scroll.vadjustment().set_value(y - dy);
            }
        ));
        self.add_controller(drag);
    }

    /// Clicking the page downloads its media, or cancels an ongoing manual
    /// download.
    fn handle_click(&self) {
        let imp = self.imp();

        match imp.loader.status() {
            FileStatus::CanBeDownloaded => imp.loader.request_download(),
            FileStatus::Downloading(_) if !imp.loader.is_auto() => imp.loader.cancel_download(),
            _ => {}
        }
    }
}

/// Checks of the media viewer page that need GTK, see `ui::tests`.
#[cfg(test)]
pub(crate) fn gtk_check() {
    eprintln!("media viewer page: video graphics offload");

    let page: MediaViewerPage = glib::Object::new();
    assert_eq!(
        page.imp().video.graphics_offload(),
        gtk::GraphicsOffloadEnabled::Enabled,
        "videos in the viewer are offloaded to the compositor"
    );

    eprintln!("media viewer page: video playback");

    crate::model::test_support::isolated(|| {
        use crate::model::test_support::*;

        let session = session();
        let path = playback_manager::tests::silent_wav("viewer-video");
        let mut video_file = file(2);
        video_file.local.path = path.to_str().unwrap().to_owned();
        video_file.local.is_downloading_completed = true;
        video_file.local.downloaded_size = video_file.size;
        let item = |is_animation| ViewerItem::Video {
            file: video_file.clone(),
            placeholder: None,
            is_animation,
        };
        let manager = playback_manager::PlaybackManager::new();
        let media = manager.media_file();
        let context = glib::MainContext::ref_thread_default();
        let wait_playing = || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !media.is_playing() && std::time::Instant::now() < deadline {
                context.iteration(false);
            }
            assert!(
                media.is_playing(),
                "the video did not start: {:?}",
                media.error()
            );
        };

        // A displayed video plays with sound on the shared stream, bound to
        // the video widget with its controls (c1eecd89).
        let page: MediaViewerPage = glib::Object::new();
        page.set_item(&session, item(false), Some(&manager));
        let imp = page.imp();
        assert!(
            !imp.video.is_visible(),
            "a video is shown before it is displayed"
        );
        page.activate();
        wait_playing();
        assert!(manager.is_current(path.to_str().unwrap()));
        assert!(!media.is_muted());
        assert!(!media.is_loop());
        assert!(imp.video.is_visible());
        assert_eq!(imp.video.media_stream().as_ref(), Some(media.upcast_ref()));
        assert!(!imp.picture.is_visible());

        // Leaving the page stops the video and shows the preview again.
        page.deactivate();
        assert!(!manager.is_current(path.to_str().unwrap()));
        assert!(!imp.video.is_visible());
        assert!(imp.picture.is_visible());

        // Animations loop and stay muted.
        page.set_item(&session, item(true), Some(&manager));
        page.activate();
        wait_playing();
        assert!(media.is_muted());
        assert!(media.is_loop());
        page.deactivate();
    });

    eprintln!("media viewer page: zooming a photo");

    crate::model::test_support::isolated(|| {
        use crate::model::test_support::*;

        let session = session();
        let page: MediaViewerPage = glib::Object::new();
        let allocate = |page: &MediaViewerPage| {
            page.measure(gtk::Orientation::Horizontal, -1);
            page.measure(gtk::Orientation::Vertical, 400);
            page.size_allocate(&gtk::Allocation::new(0, 0, 400, 300), -1);
        };
        let photo = ViewerItem::Photo {
            file: downloaded_photo_file(800),
            placeholder: None,
        };
        page.set_item(&session, photo.clone(), None);
        allocate(&page);
        let imp = page.imp();
        assert!(page.can_zoom());
        assert_eq!(page.zoom(), 1.0);

        // Zoomed in around a point, which stays under the pointer.
        page.set_zoom(2.0, Some((100.0, 100.0)));
        allocate(&page);
        assert_eq!(page.zoom(), 2.0);
        assert_eq!(imp.picture.size_request(), (800, 600));
        assert_eq!(imp.zoom_scroll.hadjustment().value(), 100.0);
        assert_eq!(imp.zoom_scroll.vadjustment().value(), 100.0);

        // Zooming further around the same point keeps it in place too.
        page.zoom_by(ZOOM_STEP, Some((100.0, 100.0)));
        allocate(&page);
        assert_eq!(page.zoom(), 2.0 * ZOOM_STEP);
        assert_eq!(imp.zoom_scroll.hadjustment().value(), 150.0);

        // A pinch keeps the photo point under the fingers, also when several
        // steps come before the next allocation.
        page.set_zoom(1.0, None);
        allocate(&page);
        assert!(page.begin_pinch((200.0, 150.0)));
        page.update_pinch(1.5, (200.0, 150.0));
        page.update_pinch(2.0, (200.0, 150.0));
        allocate(&page);
        assert_eq!(page.zoom(), 2.0);
        assert_eq!(imp.zoom_scroll.hadjustment().value(), 200.0);
        assert_eq!(imp.zoom_scroll.vadjustment().value(), 150.0);
        // Moving the fingers together moves the photo with them.
        imp.pinch_point.set(None);
        assert!(page.begin_pinch((100.0, 100.0)));
        page.update_pinch(1.0, (150.0, 130.0));
        allocate(&page);
        assert_eq!(page.zoom(), 2.0);
        assert_eq!(imp.zoom_scroll.hadjustment().value(), 150.0);
        assert_eq!(imp.zoom_scroll.vadjustment().value(), 120.0);
        imp.pinch_point.set(None);

        // Quick zoom steps before the next allocation, e.g. Ctrl+scrolling,
        // keep the point in place too.
        page.set_zoom(1.0, None);
        allocate(&page);
        page.zoom_by(ZOOM_STEP, Some((100.0, 100.0)));
        page.zoom_by(ZOOM_STEP, Some((100.0, 100.0)));
        allocate(&page);
        assert_eq!(imp.zoom_scroll.hadjustment().value(), 56.25);

        // The zoom is limited, and back at 1 the photo fits again.
        page.zoom_by(100.0, None);
        assert_eq!(page.zoom(), MAX_ZOOM);
        page.set_zoom(0.1, None);
        assert_eq!(page.zoom(), 1.0);
        assert_eq!(imp.picture.size_request(), (-1, -1));

        // Leaving the page or showing another item resets the zoom.
        page.set_zoom(3.0, None);
        page.deactivate();
        assert_eq!(page.zoom(), 1.0);
        page.set_zoom(3.0, None);
        page.set_item(&session, photo, None);
        assert_eq!(page.zoom(), 1.0);

        // Videos and photos still to be downloaded are not zoomed.
        let video = ViewerItem::Video {
            file: downloaded_photo_file(801),
            placeholder: None,
            is_animation: false,
        };
        page.set_item(&session, video, None);
        page.set_zoom(2.0, None);
        assert_eq!(page.zoom(), 1.0);
        let not_downloaded = ViewerItem::Photo {
            file: file(802),
            placeholder: None,
        };
        page.set_item(&session, not_downloaded, None);
        assert!(!page.can_zoom());
        page.set_zoom(2.0, None);
        assert_eq!(page.zoom(), 1.0);
    });
}
