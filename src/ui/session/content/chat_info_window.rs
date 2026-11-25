use std::cell::Cell;
use std::cell::OnceCell;
use std::cell::RefCell;
use std::sync::OnceLock;

use glib::subclass::Signal;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use glib::clone;
use glib::closure;
use gtk::gdk;
use gtk::glib;
use gtk::CompositeTemplate;

use crate::expressions;
use crate::i18n::ngettext_f;
use crate::model;
use crate::strings;
use crate::types::MessageId;
use crate::utils;

mod shared_media;

use shared_media::SharedMediaKind;
use shared_media::SharedMediaTile;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/chat_info_window.ui")]
    pub(crate) struct ChatInfoWindow {
        pub(super) chat: OnceCell<model::Chat>,
        #[template_child]
        pub(super) toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub(super) name_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) subtitle_label: TemplateChild<gtk::Inscription>,
        #[template_child]
        pub(super) info_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub(super) scrolled_window: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub(super) shared_media_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) shared_stack: TemplateChild<adw::ViewStack>,
        #[template_child]
        pub(super) media_grid: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub(super) files_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub(super) links_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub(super) voice_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub(super) music_list: TemplateChild<gtk::ListBox>,
        /// The loading state of every shared media section, in tab order.
        pub(super) sections: RefCell<Vec<super::Section>>,
        /// The number of tabs whose first page did not arrive yet.
        pub(super) first_pages_pending: Cell<usize>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatInfoWindow {
        const NAME: &'static str = "PaplChatInfoWindow";
        type Type = super::ChatInfoWindow;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ChatInfoWindow {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                // Emitted with the id of a shared media, file, link or voice
                // message that was activated, so that the chat history shows
                // it. The dialog closes itself right after.
                vec![Signal::builder("jump-to-message")
                    .param_types([i64::static_type()])
                    .build()]
            })
        }

        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![glib::ParamSpecObject::builder::<model::Chat>("chat")
                    .construct_only()
                    .build()]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "chat" => self.chat.set(value.get().unwrap()).unwrap(),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let obj = self.obj();

            match pspec.name() {
                "chat" => obj.chat().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup_window();
        }
    }

    impl WidgetImpl for ChatInfoWindow {}
    impl AdwDialogImpl for ChatInfoWindow {}

    #[gtk::template_callbacks]
    impl ChatInfoWindow {
        #[template_callback]
        fn on_key_pressed(
            &self,
            key: gdk::Key,
            _: u32,
            modifier: gdk::ModifierType,
            _: &gtk::EventControllerKey,
        ) -> glib::Propagation {
            if key == gdk::Key::w && modifier == gdk::ModifierType::CONTROL_MASK {
                self.obj().close();
                return glib::Propagation::Stop;
            }

            glib::Propagation::Proceed
        }

        #[template_callback]
        fn on_edge_reached(&self, pos: gtk::PositionType) {
            if pos == gtk::PositionType::Bottom {
                self.obj().load_more_of_visible_section();
            }
        }

        #[template_callback]
        fn on_visible_page_changed(&self) {
            // The new tab may be shorter than the viewport, in which case
            // scrolling to the bottom never happens.
            self.obj().fill_viewport_if_needed();
        }
    }
}

glib::wrapper! {
    pub(crate) struct ChatInfoWindow(ObjectSubclass<imp::ChatInfoWindow>)
        @extends gtk::Widget, adw::Dialog,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::ShortcutManager;
}

/// The loading state of one tab of the shared media, e.g. the files.
#[derive(Debug)]
pub(crate) struct Section {
    kind: SharedMediaKind,
    /// The id of the message to continue the search from, 0 at the start.
    next_from_message_id: Cell<MessageId>,
    is_loading: Cell<bool>,
    is_complete: Cell<bool>,
}

impl ChatInfoWindow {
    pub(crate) fn new(chat: &model::Chat) -> Self {
        glib::Object::builder().property("chat", chat).build()
    }

    /// Calls `f` with the id of a shared message the user activated, so that
    /// the chat history can show it.
    pub(crate) fn connect_jump_to_message<F: Fn(&Self, MessageId) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_closure(
            "jump-to-message",
            false,
            glib::closure_local!(move |obj: &Self, message_id: i64| f(obj, message_id)),
        )
    }

    fn setup_window(&self) {
        let imp = self.imp();
        let chat_expression = Self::this_expression("chat");

        // Bind the name
        expressions::chat_display_name(&chat_expression).bind(
            &*imp.name_label,
            "label",
            Some(self),
        );

        match self.chat().unwrap().chat_type() {
            model::ChatType::Private(user) => {
                self.setup_user_info(&user);
            }
            model::ChatType::BasicGroup(basic_group) => {
                self.setup_basic_group_info(&basic_group);
            }
            model::ChatType::Supergroup(supergroup) => {
                self.setup_supergroup_info(&supergroup);
            }
            model::ChatType::Secret(secret) => {
                self.setup_user_info(&secret.user_());
            }
        }

        self.setup_shared_media();
    }

    fn setup_user_info(&self, user: &model::User) {
        let imp = self.imp();

        // Online status or bot label
        if let tdlib::enums::UserType::Bot(_) = user.user_type().0 {
            imp.subtitle_label.set_text(Some(&gettext("bot")));
        } else {
            model::User::this_expression("status")
                .chain_closure::<String>(closure!(
                    |_: glib::Object, status: model::BoxedUserStatus| {
                        strings::user_status(&status.0)
                    }
                ))
                .bind(&*imp.subtitle_label, "text", Some(user));
        }

        // Phone number
        if !user.phone_number().is_empty() {
            let row = new_property_row(&gettext("Mobile"), &format!("+{}", user.phone_number()));
            self.make_row_copyable(&row);
            imp.info_list.append(&row);
        }

        // Username
        if !user.username().is_empty() {
            let row = new_property_row(&gettext("Username"), &format!("@{}", user.username()));
            self.make_row_copyable(&row);
            imp.info_list.append(&row);
        }

        self.update_info_list_visibility();
    }

    fn setup_basic_group_info(&self, basic_group: &model::BasicGroup) {
        let client_id = self.chat().unwrap().session_().client_().id();
        let basic_group_id = basic_group.id();
        let imp = self.imp();

        // Members number
        model::BasicGroup::this_expression("member-count")
            .chain_closure::<String>(closure!(|_: glib::Object, member_count: i32| {
                ngettext_f(
                    "{num} member",
                    "{num} members",
                    member_count as u32,
                    &[("num", &member_count.to_string())],
                )
            }))
            .bind(&*imp.subtitle_label, "text", Some(basic_group));

        self.update_info_list_visibility();

        // Full info
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let result =
                    tdlib::functions::get_basic_group_full_info(basic_group_id, client_id).await;
                match result {
                    Ok(tdlib::enums::BasicGroupFullInfo::BasicGroupFullInfo(full_info)) => {
                        obj.setup_basic_group_full_info(full_info);
                    }
                    Err(e) => {
                        log::warn!("Failed to get basic group full info: {e:?}");
                    }
                }
            }
        ));
    }

    fn setup_basic_group_full_info(&self, basic_group_full_info: tdlib::types::BasicGroupFullInfo) {
        let imp = self.imp();

        // Description
        if !basic_group_full_info.description.is_empty() {
            let row = new_property_row(&gettext("Description"), &basic_group_full_info.description);
            self.make_row_copyable(&row);
            imp.info_list.append(&row);
        }

        self.update_info_list_visibility();
    }

    fn setup_supergroup_info(&self, supergroup: &model::Supergroup) {
        let client_id = self.chat().unwrap().session_().client_().id();
        let supergroup_id = supergroup.id();
        let imp = self.imp();

        // Members number
        model::Supergroup::this_expression("member-count")
            .chain_closure::<String>(closure!(|_: glib::Object, member_count: i32| {
                ngettext_f(
                    "{num} member",
                    "{num} members",
                    member_count as u32,
                    &[("num", &member_count.to_string())],
                )
            }))
            .bind(&*imp.subtitle_label, "text", Some(supergroup));

        // Link
        if !supergroup.username().is_empty() {
            let row = new_property_row(
                &gettext("Link"),
                &format!("https://t.me/{}", supergroup.username()),
            );
            self.make_row_copyable(&row);
            imp.info_list.append(&row);
        }

        self.update_info_list_visibility();

        // Full info
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let result =
                    tdlib::functions::get_supergroup_full_info(supergroup_id, client_id).await;
                match result {
                    Ok(tdlib::enums::SupergroupFullInfo::SupergroupFullInfo(full_info)) => {
                        obj.setup_supergroup_full_info(full_info);
                    }
                    Err(e) => {
                        log::warn!("Failed to get supergroup full info: {e:?}");
                    }
                }
            }
        ));
    }

    fn setup_supergroup_full_info(&self, supergroup_full_info: tdlib::types::SupergroupFullInfo) {
        let imp = self.imp();

        // Description
        if !supergroup_full_info.description.is_empty() {
            let row = new_property_row(&gettext("Description"), &supergroup_full_info.description);
            self.make_row_copyable(&row);
            imp.info_list.append(&row);
        }

        self.update_info_list_visibility();
    }

    fn setup_shared_media(&self) {
        let imp = self.imp();

        *imp.sections.borrow_mut() = SharedMediaKind::ALL
            .iter()
            .map(|kind| Section {
                kind: *kind,
                next_from_message_id: Cell::new(0),
                is_loading: Cell::new(false),
                is_complete: Cell::new(false),
            })
            .collect();

        imp.media_grid.connect_child_activated(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, child| {
                if let Some(tile) = child.child().and_downcast::<SharedMediaTile>() {
                    obj.jump_to_message(tile.message_id());
                }
            }
        ));

        for list in [
            &*imp.files_list,
            &*imp.links_list,
            &*imp.voice_list,
            &*imp.music_list,
        ] {
            list.connect_row_activated(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, row| {
                    obj.jump_to_message(shared_media::row_message_id(row));
                }
            ));
        }

        // The first page of every tab tells whether the tab has anything at
        // all, as in Telegram, where only the tabs with content are shown.
        imp.first_pages_pending.set(SharedMediaKind::ALL.len());
        for index in 0..SharedMediaKind::ALL.len() {
            self.load_section(index);
        }
    }

    fn jump_to_message(&self, message_id: MessageId) {
        if message_id == 0 {
            return;
        }

        self.emit_by_name::<()>("jump-to-message", &[&message_id]);
        self.close();
    }

    fn page_name(kind: SharedMediaKind) -> &'static str {
        match kind {
            SharedMediaKind::Media => "media",
            SharedMediaKind::Files => "files",
            SharedMediaKind::Links => "links",
            SharedMediaKind::Voice => "voice",
            SharedMediaKind::Music => "music",
        }
    }

    fn load_more_of_visible_section(&self) {
        let imp = self.imp();
        let Some(name) = imp.shared_stack.visible_child_name() else {
            return;
        };

        let index = SharedMediaKind::ALL
            .iter()
            .position(|kind| Self::page_name(*kind) == name.as_str());

        if let Some(index) = index {
            self.load_section(index);
        }
    }

    /// Loads the next page of the visible tab while the content does not
    /// fill the viewport, as scrolling, and so loading more, is not possible
    /// then.
    fn fill_viewport_if_needed(&self) {
        let imp = self.imp();
        if !imp.shared_media_box.is_visible() {
            return;
        }

        let adjustment = imp.scrolled_window.vadjustment();
        if adjustment.upper() <= adjustment.page_size() + 1.0 {
            self.load_more_of_visible_section();
        }
    }

    fn load_section(&self, index: usize) {
        let imp = self.imp();
        let sections = imp.sections.borrow();
        let Some(section) = sections.get(index) else {
            return;
        };

        if section.is_loading.get() || section.is_complete.get() {
            return;
        }
        section.is_loading.set(true);

        let kind = section.kind;
        let from_id = section.next_from_message_id.get();
        let chat = self.chat().unwrap().clone();

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let limit = if from_id == 0 {
                    kind.first_page_size()
                } else {
                    kind.page_size()
                };

                let result = chat
                    .search_filtered_messages(kind.filter(), from_id, limit)
                    .await;

                obj.handle_section_page(index, from_id, result);
            }
        ));
    }

    fn handle_section_page(
        &self,
        index: usize,
        from_id: MessageId,
        result: Result<model::FilteredSearchPage, tdlib::types::Error>,
    ) {
        let imp = self.imp();

        'page: {
            let sections = imp.sections.borrow();
            let Some(section) = sections.get(index) else {
                return;
            };
            section.is_loading.set(false);

            let page = match result {
                Ok(page) => page,
                Err(e) => {
                    log::warn!("Failed to load the shared {:?}: {e:?}", section.kind);
                    section.is_complete.set(true);
                    break 'page;
                }
            };

            let kind = section.kind;
            let is_first_page = from_id == 0;

            // TDLib may answer the next page with the message it started from.
            let messages: Vec<_> = page
                .messages
                .into_iter()
                .filter(|message| from_id == 0 || message.id() != from_id)
                .collect();

            section.next_from_message_id.set(page.next_from_message_id);
            section
                .is_complete
                .set(page.next_from_message_id == 0 || messages.is_empty());

            for message in &messages {
                self.append_shared_message(kind, message);
            }

            if is_first_page {
                imp.shared_stack
                    .page(&self.section_widget(kind))
                    .set_visible(!messages.is_empty());
            }
        }

        if from_id == 0 {
            let pending = imp.first_pages_pending.get().saturating_sub(1);
            imp.first_pages_pending.set(pending);
            if pending == 0 {
                self.show_shared_media();
            }
        }

        // Wait for the new content to be allocated before measuring it.
        glib::idle_add_local_once(clone!(
            #[weak(rename_to = obj)]
            self,
            move || obj.fill_viewport_if_needed()
        ));
    }

    /// Shows the shared media once the first page of every tab arrived, with
    /// the first tab that has content, in tab order, selected, so that the
    /// tabs do not jump around while they arrive.
    fn show_shared_media(&self) {
        let imp = self.imp();

        let first_visible = SharedMediaKind::ALL.iter().copied().find(|kind| {
            imp.shared_stack
                .page(&self.section_widget(*kind))
                .is_visible()
        });

        let Some(first_visible) = first_visible else {
            imp.shared_media_box.set_visible(false);
            return;
        };

        imp.shared_stack
            .set_visible_child_name(Self::page_name(first_visible));
        imp.shared_media_box.set_visible(true);
    }

    fn section_widget(&self, kind: SharedMediaKind) -> gtk::Widget {
        let imp = self.imp();
        match kind {
            SharedMediaKind::Media => imp.media_grid.get().upcast(),
            SharedMediaKind::Files => imp.files_list.get().upcast(),
            SharedMediaKind::Links => imp.links_list.get().upcast(),
            SharedMediaKind::Voice => imp.voice_list.get().upcast(),
            SharedMediaKind::Music => imp.music_list.get().upcast(),
        }
    }

    fn append_shared_message(&self, kind: SharedMediaKind, message: &model::Message) {
        let imp = self.imp();

        match kind {
            SharedMediaKind::Media => {
                if let Some(tile) = SharedMediaTile::new(message) {
                    imp.media_grid.append(&tile);
                }
            }
            SharedMediaKind::Files => {
                if let Some(row) = shared_media::file_row(message) {
                    imp.files_list.append(&row);
                }
            }
            SharedMediaKind::Links => {
                imp.links_list.append(&shared_media::link_row(message));
            }
            SharedMediaKind::Voice => {
                if let Some(row) = shared_media::voice_row(message) {
                    imp.voice_list.append(&row);
                }
            }
            SharedMediaKind::Music => {
                if let Some(row) = shared_media::music_row(message) {
                    imp.music_list.append(&row);
                }
            }
        }
    }

    /// Shows the tab named `page` and activates its `index`th item, as a
    /// click does. Returns whether there was such an item. Used by the
    /// automation of the end-to-end tests.
    pub(crate) fn activate_shared_item(&self, page: &str, index: i32) -> bool {
        let imp = self.imp();
        let Some(kind) = SharedMediaKind::ALL
            .iter()
            .copied()
            .find(|kind| Self::page_name(*kind) == page)
        else {
            return false;
        };

        imp.shared_stack.set_visible_child_name(page);

        match self.section_widget(kind).downcast::<gtk::FlowBox>() {
            Ok(grid) => grid
                .child_at_index(index)
                .map(|child| child.activate())
                .is_some(),
            Err(widget) => widget
                .downcast::<gtk::ListBox>()
                .ok()
                .and_then(|list| list.row_at_index(index))
                .map(|row| row.activate())
                .is_some(),
        }
    }

    fn update_info_list_visibility(&self) {
        let info_list = &self.imp().info_list;
        info_list.set_visible(info_list.first_child().is_some());
    }

    fn make_row_copyable(&self, action_row: &adw::ActionRow) {
        action_row.set_activatable(true);
        action_row.connect_activated(clone!(
            #[weak(rename_to = obj)]
            self,
            move |action_row| {
                action_row.clipboard().set_text(&action_row.title());

                let toast = adw::Toast::new(&gettext("Copied to clipboard"));
                obj.imp().toast_overlay.add_toast(toast);
            }
        ));
    }

    pub(crate) fn chat(&self) -> Option<&model::Chat> {
        self.imp().chat.get()
    }
}

fn new_property_row(title: &str, subtitle: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .build();
    row.add_css_class("property");
    row
}
