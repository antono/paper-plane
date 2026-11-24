use std::cell::Cell;
use std::cell::RefCell;
use std::sync::OnceLock;

use glib::clone;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::CompositeTemplate;

use crate::ui;

const OBJECT_REPLACEMENT_CHARACTER: char = '\u{FFFC}';
/// Follows the spacer in the text view, see `fill_text_view`.
const WORD_JOINER: char = '\u{2060}';
const INDICATORS_SPACING: i32 = 6;

/// The size of the spacer that stands for the indicators in the text view,
/// the same room as the shape of the label reserves.
fn spacer_size(indicators_size: &gtk::Requisition) -> (i32, i32) {
    // The shape sits 3/4 above and 1/4 below the baseline, but a widget in a
    // text view has its whole height above the baseline: the full height
    // gives the line the same height as the shape does.
    let height = indicators_size.height();
    (
        indicators_size.width() + INDICATORS_SPACING,
        height - height / 4,
    )
}

/// Replaces the links of a Pango markup (`<a href="...">`) with spans whose
/// language is `x-link-N`, N being the index of the link's unescaped URI in the
/// returned list. Only the element name changes, so the nesting stays valid.
fn links_to_spans(markup: &str) -> (String, Vec<String>) {
    let mut output = String::with_capacity(markup.len());
    let mut uris = vec![];
    let mut rest = markup;

    while let Some(start) = rest.find('<') {
        output.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find('>').map_or(rest.len(), |end| end + 1);
        let tag = &rest[..end];

        if tag == "</a>" {
            output.push_str("</span>");
        } else if let Some(uri) = anchor_href(tag) {
            output.push_str(&format!("<span lang=\"x-link-{}\">", uris.len()));
            uris.push(uri);
        } else {
            output.push_str(tag);
        }
        rest = &rest[end..];
    }
    output.push_str(rest);

    (output, uris)
}

/// The unescaped `href` of an `<a ...>` tag.
fn anchor_href(tag: &str) -> Option<String> {
    let attributes = tag.strip_prefix("<a")?;
    if !attributes.starts_with(char::is_whitespace) {
        return None;
    }

    let value = attributes.split_once("href=")?.1;
    let quote = value.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let value = &value[1..];
    let value = &value[..value.find(quote)?];

    Some(
        value
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/label.ui")]
    pub(crate) struct MessageLabel {
        pub(super) text: RefCell<String>,
        pub(super) indicators: RefCell<Option<ui::MessageIndicators>>,
        pub(super) indicators_size: RefCell<Option<(i32, i32)>>,
        /// Read-only view that replaces the label while the text of a single
        /// selected message is selectable, see `set_text_selectable`.
        pub(super) text_view: RefCell<Option<gtk::TextView>>,
        /// Links of the text view as (start offset, end offset, uri).
        pub(super) links: RefCell<Vec<(i32, i32, String)>>,
        /// Reserves the room of the indicators in the text view.
        pub(super) spacer: RefCell<Option<gtk::Widget>>,
        /// Follows accent and dark/light changes while the text view exists.
        pub(super) style_handlers: RefCell<Vec<glib::SignalHandlerId>>,
        /// Whether messages are being selected, see `set_selecting`.
        pub(super) selecting: Cell<bool>,
        #[template_child]
        pub(super) label: TemplateChild<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageLabel {
        const NAME: &'static str = "PaplMessageLabel";
        type Type = super::MessageLabel;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.set_css_name("messagelabel");
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageLabel {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecString::builder("label")
                        .explicit_notify()
                        .build(),
                    glib::ParamSpecObject::builder::<ui::MessageIndicators>("indicators")
                        .construct_only()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let obj = self.obj();

            match pspec.name() {
                "label" => obj.set_label(value.get().unwrap()),
                "indicators" => obj.set_indicators(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let obj = self.obj();

            match pspec.name() {
                "label" => obj.label().to_value(),
                "indicators" => obj.indicators().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();

            // While selecting, a link must not open, whatever activates it.
            let weak = self.obj().downgrade();
            self.label.connect_activate_link(move |_, _| {
                if weak.upgrade().is_some_and(|obj| obj.is_selecting()) {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            });
        }

        fn dispose(&self) {
            self.obj().disconnect_style_handlers();
            if let Some(text_view) = self.text_view.take() {
                text_view.unparent();
            }
            self.label.unparent();
            if let Some(indicators) = self.indicators.take() {
                indicators.unparent();
            }
        }
    }

    impl WidgetImpl for MessageLabel {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let obj = self.obj();

            if let Some(indicators) = self.indicators.borrow().as_ref() {
                let (_, indicators_size) = indicators.preferred_size();
                let old = self
                    .indicators_size
                    .replace(Some((indicators_size.width(), indicators_size.height())));

                if let Some(old_indicators_size) = old {
                    if indicators_size.width() != old_indicators_size.0
                        || indicators_size.height() != old_indicators_size.1
                    {
                        obj.update_label_attributes(&indicators_size);
                    }
                } else {
                    obj.update_label_attributes(&indicators_size);
                }

                let (mut minimum, mut natural, minimum_baseline, natural_baseline) =
                    self.label.measure(orientation, for_size);
                let (indicators_min, indicators_nat, _, _) =
                    indicators.measure(orientation, for_size);

                minimum = minimum.max(indicators_min);
                natural = natural.max(indicators_nat);

                if orientation == gtk::Orientation::Vertical && obj.is_opposite_text_direction() {
                    minimum += indicators_min;
                    natural += indicators_nat;
                }

                (minimum, natural, minimum_baseline, natural_baseline)
            } else {
                self.label.measure(orientation, for_size)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.label.allocate(width, height, baseline, None);
            if let Some(text_view) = self.text_view.borrow().as_ref() {
                // GTK expects every allocation to follow a measurement
                text_view.measure(gtk::Orientation::Horizontal, -1);
                text_view.measure(gtk::Orientation::Vertical, width);
                text_view.allocate(width, height, baseline, None);
            }
            if let Some(indicators) = self.indicators.borrow().as_ref() {
                indicators.allocate(width, height, baseline, None);
            }
        }

        fn request_mode(&self) -> gtk::SizeRequestMode {
            self.label.request_mode()
        }

        fn direction_changed(&self, _previous_direction: gtk::TextDirection) {
            self.obj().update_label();
        }
    }
}

glib::wrapper! {
    pub(crate) struct MessageLabel(ObjectSubclass<imp::MessageLabel>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MessageLabel {
    fn update_label_attributes(&self, indicators_size: &gtk::Requisition) {
        let imp = self.imp();
        if let Some(spacer) = imp.spacer.borrow().as_ref() {
            let (width, height) = spacer_size(indicators_size);
            spacer.set_size_request(width, height);
        }
        if let Some(start_index) = imp.label.text().find(OBJECT_REPLACEMENT_CHARACTER) {
            let attrs = pango::AttrList::new();
            let width = indicators_size.width() + INDICATORS_SPACING;
            let height = indicators_size.height();
            let logical_rect = pango::Rectangle::new(
                0,
                -(height - (height / 4)) * pango::SCALE,
                width * pango::SCALE,
                height * pango::SCALE,
            );
            let mut shape = pango::AttrShape::new(&logical_rect, &logical_rect);

            shape.set_start_index(start_index as u32);
            shape.set_end_index((start_index + OBJECT_REPLACEMENT_CHARACTER.len_utf8()) as u32);
            attrs.insert(shape);

            imp.label.set_attributes(Some(&attrs));
        } else {
            imp.label.set_attributes(None);
        }
    }

    fn is_opposite_text_direction(&self) -> bool {
        let text = self.imp().text.borrow();
        let text_direction = pango::find_base_dir(&text);
        let widget_direction = self.direction();

        (text_direction == pango::Direction::Rtl && widget_direction == gtk::TextDirection::Ltr)
            || text_direction == pango::Direction::Ltr
                && widget_direction == gtk::TextDirection::Rtl
    }

    fn update_label(&self) {
        let imp = self.imp();
        self.fill_text_view();
        let text = imp.text.borrow();
        if let Some(indicators) = imp.indicators.borrow().as_ref() {
            if !self.is_opposite_text_direction() {
                imp.label
                    .set_label(&format!("{text}{OBJECT_REPLACEMENT_CHARACTER}"));
            } else {
                imp.label.set_label(&text);
            }

            let (_, indicators_size) = indicators.preferred_size();
            self.update_label_attributes(&indicators_size);
        } else {
            imp.label.set_label(&text);
        }
    }

    pub(crate) fn label(&self) -> String {
        self.imp().text.borrow().clone()
    }

    /// Returns the plain text of the label, without markup and without the
    /// object replacement character used for the indicators.
    pub(crate) fn plain_text(&self) -> String {
        let imp = self.imp();
        if let Some(text_view) = imp.text_view.borrow().as_ref() {
            let buffer = text_view.buffer();
            let (start, end) = buffer.bounds();
            return buffer
                .text(&start, &end, false)
                .replace([OBJECT_REPLACEMENT_CHARACTER, WORD_JOINER], "");
        }

        imp.label.text().replace(OBJECT_REPLACEMENT_CHARACTER, "")
    }

    /// Returns the currently selected plain text, if any.
    pub(crate) fn selected_text(&self) -> Option<String> {
        let imp = self.imp();
        if let Some(text_view) = imp.text_view.borrow().as_ref() {
            let buffer = text_view.buffer();
            let (start, end) = buffer.selection_bounds()?;
            let text = buffer
                .text(&start, &end, false)
                .replace([OBJECT_REPLACEMENT_CHARACTER, WORD_JOINER], "");
            return (!text.is_empty()).then_some(text);
        }
        let (start, end) = imp.label.selection_bounds()?;

        let text: String = imp
            .label
            .text()
            .chars()
            .skip(start as usize)
            .take((end - start) as usize)
            .collect();
        let text = text.replace(OBJECT_REPLACEMENT_CHARACTER, "");

        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    pub(crate) fn set_label(&self, label: String) {
        let imp = self.imp();
        let old = imp.text.replace(label);
        if old != *imp.text.borrow() {
            self.update_label();
            self.notify("label");
        }
    }

    /// Lets taps through the label while messages are being selected. A
    /// `GtkLabel` with links claims every press on a link, selectable or not,
    /// which would deny the row's gesture and open the link instead of
    /// toggling the message.
    pub(crate) fn set_selecting(&self, selecting: bool) {
        let imp = self.imp();
        imp.selecting.set(selecting);
        imp.label.set_can_target(!selecting);
    }

    pub(crate) fn is_selecting(&self) -> bool {
        self.imp().selecting.get()
    }

    /// Whether the label below takes clicks, and whether it lets a link open.
    #[cfg(test)]
    pub(crate) fn links_work(&self) -> (bool, bool) {
        let label = &self.imp().label;
        let stopped = label.emit_by_name::<bool>("activate-link", &[&"https://example.com"]);
        (label.can_target(), !stopped)
    }

    pub(crate) fn text_view(&self) -> Option<gtk::TextView> {
        self.imp().text_view.borrow().clone()
    }

    /// Makes the text selectable (with touch handles and a magnifier on touch
    /// screens), or back to a plain label.
    ///
    /// A selectable `GtkLabel` claims the touch sequence on press, so
    /// scrolling the history would select text instead. A `GtkTextView` is
    /// used only for the one message that is selected, and it replaces the
    /// label on screen while the label stays in the widget tree: a text view
    /// ignores the width it is measured for, the label does not, so the label
    /// keeps deciding the size of the row.
    pub(crate) fn set_text_selectable(&self, selectable: bool) {
        let imp = self.imp();
        if selectable == imp.text_view.borrow().is_some() {
            return;
        }

        if selectable {
            let text_view = gtk::TextView::new();
            text_view.set_editable(false);
            text_view.set_cursor_visible(false);
            // The press handler's `grab_focus` fails, the composer keeps focus.
            text_view.set_focusable(false);
            text_view.set_wrap_mode(gtk::WrapMode::WordChar);
            text_view.set_left_margin(0);
            text_view.set_right_margin(0);
            text_view.set_top_margin(0);
            text_view.set_bottom_margin(0);
            text_view.set_pixels_above_lines(0);
            text_view.set_extra_menu(gio::MenuModel::NONE);
            text_view.add_css_class("message-text-view");

            let click = gtk::GestureClick::builder().button(1).build();
            click.connect_released(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _, x, y| obj.activate_link_at(x, y)
            ));
            text_view.add_controller(click);

            // The link tags hold a color, not a style, so they are refreshed
            // by hand
            let style_manager = adw::StyleManager::default();
            let handlers = ["notify::accent-color", "notify::dark"].map(|signal| {
                style_manager.connect_notify_local(
                    Some(&signal["notify::".len()..]),
                    clone!(
                        #[weak(rename_to = obj)]
                        self,
                        move |_, _| obj.refresh_link_colors()
                    ),
                )
            });
            imp.style_handlers.replace(handlers.into());

            text_view.set_parent(self);
            imp.text_view.replace(Some(text_view));
            imp.label.set_child_visible(false);
            self.fill_text_view();
        } else {
            if let Some(text_view) = imp.text_view.take() {
                text_view.unparent();
            }
            self.disconnect_style_handlers();
            imp.links.take();
            imp.spacer.take();
            imp.label.set_child_visible(true);
        }
        self.queue_resize();
    }

    #[cfg(test)]
    pub(crate) fn link_ranges(&self) -> Vec<(i32, i32, String)> {
        self.imp().links.borrow().clone()
    }

    /// Puts the text with its links into the text view, if there is one.
    fn fill_text_view(&self) {
        let imp = self.imp();
        let Some(text_view) = imp.text_view.borrow().clone() else {
            return;
        };

        let buffer = text_view.buffer();
        buffer.set_text("");
        // The tags of the previous text are not needed any more
        let tag_table = buffer.tag_table();
        let mut old_tags = vec![];
        tag_table.foreach(|tag| old_tags.push(tag.clone()));
        for tag in old_tags {
            tag_table.remove(&tag);
        }

        // `insert_markup` has no links, so they travel as the language of a
        // span, which becomes the `language` property of a tag.
        let (markup, uris) = links_to_spans(&imp.text.borrow());
        buffer.insert_markup(&mut buffer.end_iter(), &markup);

        let mut links = vec![];
        let mut open: Vec<(gtk::TextTag, i32)> = vec![];
        let link_index = |tag: &gtk::TextTag| {
            tag.property::<Option<String>>("language")
                .and_then(|language| language.strip_prefix("x-link-")?.parse::<usize>().ok())
        };
        let mut iter = buffer.start_iter();
        loop {
            for tag in iter.toggled_tags(true) {
                if link_index(&tag).is_some() {
                    tag.set_underline(pango::Underline::Single);
                    open.push((tag, iter.offset()));
                }
            }
            for tag in iter.toggled_tags(false) {
                if let Some(position) = open.iter().position(|(open, _)| *open == tag) {
                    let (tag, start) = open.remove(position);
                    if let Some(uri) = link_index(&tag).and_then(|index| uris.get(index)) {
                        links.push((start, iter.offset(), uri.clone()));
                    }
                }
            }
            if !iter.forward_to_tag_toggle(gtk::TextTag::NONE) {
                break;
            }
        }
        let end = buffer.end_iter().offset();
        for (tag, start) in open {
            if let Some(uri) = link_index(&tag).and_then(|index| uris.get(index)) {
                links.push((start, end, uri.clone()));
            }
        }
        imp.links.replace(links);
        self.refresh_link_colors();

        // Room for the time and ticks, like the shape of the label.
        imp.spacer.take();
        if let Some(indicators) = imp.indicators.borrow().as_ref() {
            if !self.is_opposite_text_direction() {
                let (_, indicators_size) = indicators.preferred_size();
                let (width, height) = spacer_size(&indicators_size);
                let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                spacer.set_size_request(width, height);
                let anchor = buffer.create_child_anchor(&mut buffer.end_iter());
                text_view.add_child_at_anchor(&spacer, &anchor);
                // A widget has no descent, so a line with nothing but the
                // spacer would be shorter than one with the shape of the label.
                // An invisible character that cannot wrap brings the descent
                // of the font to the line.
                buffer.insert(&mut buffer.end_iter(), &WORD_JOINER.to_string());
                imp.spacer.replace(Some(spacer.upcast()));
            }
        }
        self.queue_resize();
    }

    fn disconnect_style_handlers(&self) {
        let style_manager = adw::StyleManager::default();
        for handler in self.imp().style_handlers.take() {
            style_manager.disconnect(handler);
        }
    }

    /// Applies the current link color to the link tags of the text view.
    fn refresh_link_colors(&self) {
        let Some(text_view) = self.imp().text_view.borrow().clone() else {
            return;
        };
        let color = self.link_color();
        text_view.buffer().tag_table().foreach(|tag| {
            let is_link = tag
                .property::<Option<String>>("language")
                .is_some_and(|language| language.starts_with("x-link-"));
            if is_link {
                tag.set_foreground_rgba(Some(&color));
            }
        });
    }

    /// The text of the links: the accent color, unless the bubble is
    /// outgoing, where the accent is the background.
    fn link_color(&self) -> gdk::RGBA {
        let outgoing = self
            .ancestor(ui::MessageBubble::static_type())
            .is_some_and(|bubble| bubble.has_css_class("outgoing"));
        if outgoing {
            self.color()
        } else {
            adw::StyleManager::default().accent_color_rgba()
        }
    }

    fn link_at(&self, x: f64, y: f64) -> Option<String> {
        let text_view = self.imp().text_view.borrow().clone()?;
        let buffer = text_view.buffer();
        if buffer.has_selection() {
            return None;
        }

        let (x, y) =
            text_view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let offset = text_view.iter_at_location(x, y)?.offset();
        self.imp()
            .links
            .borrow()
            .iter()
            .find(|(start, end, _)| *start <= offset && offset < *end)
            .map(|(_, _, uri)| uri.clone())
    }

    /// Opens the link under the point, unless the message is selected: a click
    /// then belongs to the selection.
    fn activate_link_at(&self, x: f64, y: f64) {
        let selection_active = self
            .ancestor(ui::MessageRow::static_type())
            .and_downcast::<ui::MessageRow>()
            .is_some_and(|row| row.selection_active());
        if selection_active {
            return;
        }

        if let Some(uri) = self.link_at(x, y) {
            gtk::UriLauncher::new(&uri).launch(
                self.root().and_downcast_ref::<gtk::Window>(),
                gio::Cancellable::NONE,
                |_| {},
            );
        }
    }

    pub(crate) fn add_label_class(&self, class: &str) {
        self.imp().label.add_css_class(class);
    }

    pub(crate) fn indicators(&self) -> Option<ui::MessageIndicators> {
        self.imp().indicators.borrow().clone()
    }

    pub(crate) fn set_indicators(&self, indicators: Option<ui::MessageIndicators>) {
        let imp = self.imp();
        let old = imp.indicators.replace(indicators);
        if old != *imp.indicators.borrow() {
            if let Some(old) = old {
                old.unparent();
            }

            if let Some(indicators) = imp.indicators.borrow().as_ref() {
                indicators.set_parent(self);
            }

            self.update_label();
            self.notify("indicators");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::test_support::isolated;

    #[test]
    fn links_to_spans() {
        isolated(|| {
            let (markup, uris) = super::links_to_spans(
                "a <a href='https://x.org/?a=1&amp;b=2'>one <b>bold</b></a> and \
                 <a href=\"tel:137\">two</a> <i>end</i>",
            );
            assert_eq!(
                markup,
                "a <span lang=\"x-link-0\">one <b>bold</b></span> and \
                 <span lang=\"x-link-1\">two</span> <i>end</i>"
            );
            assert_eq!(uris, ["https://x.org/?a=1&b=2", "tel:137"]);
            pango::parse_markup(&markup, '\0').expect("valid markup");

            let (markup, uris) = super::links_to_spans("plain &lt;a href='x'&gt;");
            assert_eq!(markup, "plain &lt;a href='x'&gt;");
            assert!(uris.is_empty());
        });
    }
}

/// Checks of Select and Copy Text that need GTK, see `ui::tests`.
#[cfg(test)]
pub(crate) fn gtk_check() {
    text_view_gtk_check();

    eprintln!("message labels are not selectable");

    let label: MessageLabel = glib::Object::new();
    // A selectable label claims the touch sequence on press, so scrolling the
    // history on a touch screen would select text instead.
    assert!(!label.imp().label.is_selectable());

    eprintln!("message label text selection");

    // `selected_text` still reports a selection, e.g. of a selectable view.
    label.imp().label.set_selectable(true);
    label.set_label("Hello <b>wide</b> world".to_owned());
    assert_eq!(
        label.plain_text(),
        "Hello wide world",
        "markup is not copied"
    );
    assert_eq!(label.selected_text(), None);

    label.imp().label.select_region(6, 10);
    assert_eq!(label.selected_text().as_deref(), Some("wide"));

    // The indicators (time, ticks) sit on an object replacement character,
    // which must not end up in the clipboard.
    let indicators: ui::MessageIndicators = glib::Object::new();
    label.set_indicators(Some(indicators));
    assert!(!label.plain_text().contains(OBJECT_REPLACEMENT_CHARACTER));
    assert!(label.plain_text().starts_with("Hello wide world"));
    let length = label.imp().label.text().chars().count() as i32;
    label.imp().label.select_region(0, length);
    assert_eq!(
        label.selected_text().as_deref().map(str::trim_end),
        Some("Hello wide world")
    );
}

/// The text view of a single selected message.
#[cfg(test)]
fn text_view_gtk_check() {
    use crate::model::test_support::isolated;

    let formatted = |text: &str, url: &str, offset: i32, length: i32| {
        crate::utils::parse_formatted_text(tdlib::types::FormattedText {
            text: text.to_owned(),
            entities: vec![tdlib::types::TextEntity {
                offset,
                length,
                r#type: tdlib::enums::TextEntityType::TextUrl(
                    tdlib::types::TextEntityTypeTextUrl {
                        url: url.to_owned(),
                    },
                ),
            }],
        })
    };
    let label_with_indicators = |markup: String| {
        let label: MessageLabel = glib::Object::new();
        label.set_indicators(Some(glib::Object::new()));
        label.set_label(markup);
        label
    };

    isolated(|| {
        eprintln!("message text view: swap, read-only and focus");
        let url = "https://example.org/?a=1&b=2";
        let label = label_with_indicators(formatted("Hello wide world", url, 6, 4));
        assert!(label.text_view().is_none());
        label.set_text_selectable(true);
        let text_view = label.text_view().expect("a text view");
        assert!(!text_view.is_editable());
        assert!(!text_view.is_cursor_visible());
        assert!(!text_view.is_focusable());
        assert!(!label.imp().label.is_child_visible());
        assert!(text_view.parent() == Some(label.clone().upcast()));
        let buffer = text_view.buffer();
        let (start, end) = buffer.bounds();
        assert_eq!(
            buffer
                .text(&start, &end, false)
                .replace([OBJECT_REPLACEMENT_CHARACTER, WORD_JOINER], ""),
            label.plain_text()
        );
        assert_eq!(label.plain_text(), "Hello wide world");
        assert!(label.imp().spacer.borrow().is_some(), "room for indicators");

        eprintln!("message text view: links keep their targets");
        assert_eq!(label.link_ranges(), [(6, 10, url.to_owned())]);
        // The target is found at the position of the link only
        let (x, y) = (0.0, 0.0);
        assert_eq!(label.link_at(x, y), None);
        // An edit while selected refills the view
        label.set_label(formatted("Hi wide world", url, 3, 4));
        assert_eq!(label.link_ranges(), [(3, 7, url.to_owned())]);

        label.set_text_selectable(false);
        assert!(label.text_view().is_none());
        assert!(label.imp().label.is_child_visible());
        assert!(label.link_ranges().is_empty());
        assert!(text_view.parent().is_none());
    });

    isolated(|| {
        eprintln!("message text view: same height as the label");
        let context = glib::MainContext::default();
        for width in [200, 400, 233, 263, 291, 317] {
            let window = gtk::Window::new();
            window.set_default_size(width, 300);
            let label = label_with_indicators(
                "A long enough message with <b>some bold</b> words that wraps on narrow \
                 widths, and <a href='https://example.org'>a link</a> too."
                    .to_owned(),
            );
            label.set_valign(gtk::Align::Start);
            window.set_child(Some(&label));
            window.present();
            let settle = |condition: &dyn Fn() -> bool| {
                for _ in 0..200 {
                    if condition() {
                        break;
                    }
                    context.iteration(false);
                    // The frame clock ticks in real time
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                assert!(condition(), "the layout settles at width {width}");
            };
            settle(&|| label.is_mapped() && label.width() > 0);
            // The window may not give exactly the requested width
            let width = label.width();
            eprintln!("  label width {width}");

            let height = |label: &MessageLabel| label.measure(gtk::Orientation::Vertical, width).1;
            let before = height(&label);
            label.set_text_selectable(true);
            let after = height(&label);
            assert!(
                (before - after).abs() <= 2,
                "width {width}: {before} vs {after}"
            );

            // The view lays out its own text (word-char wrap, a spacer
            // widget instead of the shape): it must fit the allocation.
            let text_view = label.text_view().unwrap();
            settle(&|| text_view.height() > 0);
            let buffer = text_view.buffer();
            let (line_y, line_height) = text_view.line_yrange(&buffer.end_iter());
            let laid_out = line_y + line_height;
            assert!(
                laid_out <= after + 2,
                "width {width}: laid out {laid_out} vs {after}"
            );
            assert!(
                (laid_out - after).abs() <= 2,
                "width {width}: laid out {laid_out} vs label {after}"
            );

            // The lookup finds the link at its position, and only there
            let text_view = label.text_view().unwrap();
            settle(&|| text_view.is_mapped());
            let buffer = text_view.buffer();
            let (start, _, uri) = label.link_ranges()[0].clone();
            let rect = text_view.iter_location(&buffer.iter_at_offset(start));
            let (x, y) =
                text_view.buffer_to_window_coords(gtk::TextWindowType::Widget, rect.x(), rect.y());
            assert_eq!(
                label.link_at(x as f64 + 1.0, y as f64 + 1.0).as_deref(),
                Some(uri.as_str()),
                "width {width}: link found at its start"
            );
            assert_eq!(label.link_at(0.0, 0.0), None, "outside the link");
            // A selection belongs to the user, a click does not open links
            buffer.select_range(&buffer.iter_at_offset(0), &buffer.iter_at_offset(3));
            assert_eq!(label.link_at(x as f64 + 1.0, y as f64 + 1.0), None);
            buffer.place_cursor(&buffer.start_iter());

            window.set_child(None::<&gtk::Widget>);
            window.destroy();
        }
    });

    isolated(|| {
        eprintln!("message text view: selection is copied");
        let label = label_with_indicators("Hello <b>wide</b> world".to_owned());
        label.set_text_selectable(true);
        assert_eq!(label.selected_text(), None);

        let buffer = label.text_view().unwrap().buffer();
        buffer.select_range(&buffer.iter_at_offset(6), &buffer.iter_at_offset(10));
        assert_eq!(label.selected_text().as_deref(), Some("wide"));
    });
}
