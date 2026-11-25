use gtk::gdk;
use gtk::glib;
use gtk::graphene;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::CompositeTemplate;

use crate::model;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/base.ui")]
    pub(crate) struct MessageBase {
        #[template_child]
        pub(super) right_click: TemplateChild<gtk::GestureClick>,
        #[template_child]
        pub(super) right_long_press: TemplateChild<gtk::GestureLongPress>,
        #[template_child]
        pub(super) tap: TemplateChild<gtk::GestureClick>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageBase {
        const NAME: &'static str = "PaplMessageBase";
        const ABSTRACT: bool = true;
        type Type = super::MessageBase;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[gtk::template_callbacks]
    impl MessageBase {
        #[template_callback]
        fn on_right_click_pressed(&self) {
            // Claim the event sequence so that the label below doesn't
            // receive the press (it would show its own context menu).
            self.right_click.set_state(gtk::EventSequenceState::Claimed);
        }

        #[template_callback]
        fn on_pressed(&self, _n_press: i32, x: f64, y: f64) {
            self.show_message_menu(x as i32, y as i32);
        }

        #[template_callback]
        fn on_tap_released(&self, n_press: i32, x: f64, y: f64) {
            if n_press != 1 {
                return;
            }
            let obj = &*self.obj();
            // In the selection mode a tap selects, see `Row`.
            let row = utils::ancestor::<_, ui::MessageRow>(obj);
            if row.selection_active() || !super::tap_opens_menu(obj.upcast_ref(), x, y) {
                return;
            }
            self.tap.set_state(gtk::EventSequenceState::Claimed);
            self.show_message_menu(x as i32, y as i32);
        }

        #[template_callback]
        fn on_long_pressed(&self, x: f64, y: f64) {
            // Claiming also cancels the selection click of the row, so the
            // release doesn't toggle the message back.
            self.right_long_press
                .set_state(gtk::EventSequenceState::Claimed);

            let row = utils::ancestor::<_, ui::MessageRow>(&*self.obj());
            if self.select_word_at(&row, x, y) {
                return;
            }
            let can_select = row.message().is::<model::Message>();
            if can_select && !(row.selection_active() && row.is_selected()) {
                // Enters the selection mode, or adds to the selection
                row.activate_action("message-row.select", None).unwrap();
            } else {
                // A message that is already selected keeps its menu
                // reachable on touch (React, Edit).
                self.show_message_menu(x as i32, y as i32);
            }
        }

        /// A long press on the selectable text selects the word under it, like
        /// in Telegram, instead of opening the menu.
        fn select_word_at(&self, row: &ui::MessageRow, x: f64, y: f64) -> bool {
            let Some(text_view) = row.active_text_view() else {
                return false;
            };
            let Some(point) = self
                .obj()
                .compute_point(&text_view, &graphene::Point::new(x as f32, y as f32))
            else {
                return false;
            };
            let (x, y) = (point.x() as i32, point.y() as i32);
            if x < 0 || y < 0 || x >= text_view.width() || y >= text_view.height() {
                return false;
            }

            let (x, y) = text_view.window_to_buffer_coords(gtk::TextWindowType::Widget, x, y);
            let Some(mut start) = text_view.iter_at_location(x, y) else {
                return false;
            };
            if !start.starts_word() {
                start.backward_word_start();
            }
            let mut end = start;
            end.forward_word_end();
            text_view.buffer().select_range(&start, &end);

            true
        }

        pub(super) fn show_message_menu(&self, x: i32, y: i32) {
            let obj = &*self.obj();
            let row = utils::ancestor::<_, ui::MessageRow>(obj);
            row.prepare_message_menu();

            let chat_history = utils::ancestor::<_, ui::ChatHistory>(obj);
            chat_history
                .quick_reactions()
                .set_message(row.message().downcast_ref::<model::Message>());
            let menu = chat_history.message_menu();

            menu.set_pointing_to(Some(&gdk::Rectangle::new(x, y, 0, 0)));
            menu.unparent();
            menu.set_parent(obj);
            menu.popup();
        }
    }

    impl ObjectImpl for MessageBase {
        fn dispose(&self) {
            let mut child = self.obj().first_child();
            while let Some(child_) = child {
                child = child_.next_sibling();
                child_.unparent();
            }
        }
    }

    impl WidgetImpl for MessageBase {}
}

glib::wrapper! {
    pub(crate) struct MessageBase(ObjectSubclass<imp::MessageBase>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

/// Whether a tap at `x`, `y` of a message opens its menu: not on a button
/// (reactions, play, download) or on anything else that handles a click itself
/// (media, the replied message). A label or text view claims the tap itself if
/// it hits a link.
fn tap_opens_menu(base: &gtk::Widget, x: f64, y: f64) -> bool {
    let Some(mut widget) = base.pick(x, y, gtk::PickFlags::DEFAULT) else {
        return false;
    };
    while &widget != base {
        // Buttons have a click gesture too.
        if !widget.is::<gtk::Label>() && !widget.is::<gtk::TextView>() && handles_clicks(&widget) {
            return false;
        }
        match widget.parent() {
            Some(parent) => widget = parent,
            None => return false,
        }
    }
    true
}

/// Whether the widget has its own gesture for primary clicks.
fn handles_clicks(widget: &gtk::Widget) -> bool {
    widget
        .observe_controllers()
        .snapshot()
        .into_iter()
        .filter_map(|controller| controller.downcast::<gtk::GestureClick>().ok())
        .any(|click| matches!(click.button(), 0 | 1))
}

impl MessageBase {
    /// Opens the menu of the message at its center, as a tap does.
    pub(crate) fn open_menu(&self) {
        self.imp()
            .show_message_menu(self.width() / 2, self.height() / 2);
    }
}

pub(crate) trait MessageBaseExt:
    glib::object::IsClass + IsA<glib::Object> + IsA<gtk::Widget> + IsA<MessageBase>
{
    type Message: IsA<glib::Object>;

    fn new(message: &Self::Message) -> Self {
        glib::Object::builder().property("message", message).build()
    }

    fn set_message(&self, message: &Self::Message);
}

pub(crate) trait MessageBaseImpl: WidgetImpl + ObjectImpl + 'static {}

unsafe impl<T: MessageBaseImpl> IsSubclassable<T> for MessageBase {
    fn class_init(class: &mut glib::Class<Self>) {
        Self::parent_class_init::<T>(class.upcast_ref_mut());
    }
}

/// Where a tap opens the message menu, see `tap_opens_menu`.
#[cfg(test)]
pub(crate) fn gtk_check() {
    use gtk::glib;

    eprintln!("message menu: taps on clickable parts are theirs");

    crate::model::test_support::isolated(|| {
        let base = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let clickable = |button: u32| {
            let widget = gtk::Box::builder().height_request(20).build();
            widget.add_controller(gtk::GestureClick::builder().button(button).build());
            base.append(&widget);
            widget
        };
        let text = gtk::Label::new(Some("text"));
        base.append(&text);
        let media = clickable(1);
        let any_button = clickable(0);
        let secondary = clickable(3);
        let button = gtk::Button::with_label("button");
        base.append(&button);

        let window = gtk::Window::new();
        window.set_child(Some(&base));
        window.present();
        let context = glib::MainContext::default();
        for _ in 0..200 {
            if base.is_mapped() && base.height() > 0 {
                break;
            }
            context.iteration(false);
        }

        let opens = |widget: &gtk::Widget| {
            let bounds = widget.compute_bounds(&base).unwrap();
            tap_opens_menu(
                base.upcast_ref(),
                (bounds.x() + bounds.width() / 2.0) as f64,
                (bounds.y() + bounds.height() / 2.0) as f64,
            )
        };
        assert!(opens(text.upcast_ref()), "plain text opens the menu");
        assert!(!opens(media.upcast_ref()), "media handle their clicks");
        assert!(!opens(any_button.upcast_ref()), "so do gestures for any button");
        assert!(opens(secondary.upcast_ref()), "a right click gesture is not a tap");
        assert!(!opens(button.upcast_ref()), "buttons handle their clicks");
        assert!(!tap_opens_menu(base.upcast_ref(), -10.0, -10.0), "outside");

        window.destroy();
    });
}
