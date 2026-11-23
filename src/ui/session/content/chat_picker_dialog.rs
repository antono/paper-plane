use std::cell::OnceCell;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::subclass::Signal;
use gtk::glib;
use gtk::CompositeTemplate;

use crate::expressions;
use crate::model;
use crate::ui;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/chat_picker_dialog.ui")]
    pub(crate) struct ChatPickerDialog {
        pub(super) filter: OnceCell<gtk::StringFilter>,
        pub(super) filtered: OnceCell<gtk::FilterListModel>,
        #[template_child]
        pub(super) search_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub(super) stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) list_view: TemplateChild<gtk::ListView>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatPickerDialog {
        const NAME: &'static str = "PaplChatPickerDialog";
        type Type = super::ChatPickerDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[gtk::template_callbacks]
    impl ChatPickerDialog {
        #[template_callback]
        fn on_search_changed(&self) {
            self.obj().set_search(&self.search_entry.text());
        }

        /// Enter picks the first chat of the (filtered) list.
        #[template_callback]
        fn on_search_activate(&self) {
            self.obj().pick(0);
        }

        #[template_callback]
        fn on_activate(&self, position: u32) {
            self.obj().pick(position);
        }
    }

    impl ObjectImpl for ChatPickerDialog {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![Signal::builder("chat-picked")
                    .param_types([model::Chat::static_type()])
                    .build()]
            })
        }
    }

    impl WidgetImpl for ChatPickerDialog {}
    impl AdwDialogImpl for ChatPickerDialog {}
}

glib::wrapper! {
    pub(crate) struct ChatPickerDialog(ObjectSubclass<imp::ChatPickerDialog>)
        @extends gtk::Widget, adw::Dialog,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ChatPickerDialog {
    pub(crate) fn new(session: &model::ClientStateSession) -> Self {
        let obj: Self = glib::Object::new();
        obj.setup_model(session);
        obj
    }

    fn setup_model(&self, session: &model::ClientStateSession) {
        let imp = self.imp();

        // The title of the chat of a chat list item.
        let title_expression = gtk::PropertyExpression::new(
            model::ChatListItem::static_type(),
            None::<gtk::Expression>,
            "chat",
        )
        .chain_property::<model::Chat>("title");
        let filter = gtk::StringFilter::builder()
            .match_mode(gtk::StringFilterMatchMode::Substring)
            .ignore_case(true)
            .expression(title_expression)
            .build();
        let filtered =
            gtk::FilterListModel::new(Some(session.main_chat_list()), Some(filter.clone()));

        filtered.connect_items_changed(glib::clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, _, _, _| obj.update_stack()
        ));

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, list_item| {
            let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
            let avatar: ui::Avatar = glib::Object::builder().property("size", 32).build();
            let label = gtk::Label::builder()
                .hexpand(true)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            let row = gtk::Box::builder().spacing(12).build();
            row.append(&avatar);
            row.append(&label);
            list_item.set_child(Some(&row));
        });
        factory.connect_bind(|_, list_item| {
            let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
            let Some(chat) = list_item
                .item()
                .and_downcast::<model::ChatListItem>()
                .map(|item| item.chat_())
            else {
                return;
            };
            let row = list_item.child().and_downcast::<gtk::Box>().unwrap();
            let avatar = row.first_child().and_downcast::<ui::Avatar>().unwrap();
            let label = row.last_child().and_downcast::<gtk::Label>().unwrap();

            avatar.set_item(Some(chat.clone().upcast()));
            // Own chat and deleted accounts are not named by their title.
            let name = expressions::chat_display_name(&gtk::ConstantExpression::new(&chat))
                .evaluate_as::<String, glib::Object>(None)
                .unwrap_or_else(|| chat.title());
            label.set_label(&name);
        });
        factory.connect_unbind(|_, list_item| {
            let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(avatar) = list_item
                .child()
                .and_then(|row| row.first_child())
                .and_downcast::<ui::Avatar>()
            {
                avatar.set_item(None);
            }
        });

        imp.list_view.set_factory(Some(&factory));
        imp.list_view
            .set_model(Some(&gtk::NoSelection::new(Some(filtered.clone()))));

        imp.filter.set(filter).unwrap();
        imp.filtered.set(filtered).unwrap();
        self.update_stack();
    }

    /// Shows only the chats whose title contains the text.
    fn set_search(&self, text: &str) {
        if let Some(filter) = self.imp().filter.get() {
            filter.set_search(Some(text));
        }
    }

    fn update_stack(&self) {
        let imp = self.imp();
        let empty = imp.filtered.get().is_none_or(|model| model.n_items() == 0);
        imp.stack
            .set_visible_child_name(if empty { "empty" } else { "list" });
    }

    /// Emits `chat-picked` for the chat at the position of the filtered list
    /// and closes the dialog.
    fn pick(&self, position: u32) {
        let chat = self
            .imp()
            .filtered
            .get()
            .and_then(|model| model.item(position))
            .and_downcast::<model::ChatListItem>()
            .map(|item| item.chat_());

        if let Some(chat) = chat {
            self.emit_by_name::<()>("chat-picked", &[&chat]);
            self.close();
        }
    }

    pub(crate) fn connect_chat_picked<F: Fn(&Self, model::Chat) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("chat-picked", true, move |values| {
            let obj = values[0].get().unwrap();
            let chat = values[1].get().unwrap();
            f(obj, chat);
            None
        })
    }
}

/// The chat picker (forwarding), see `ui::tests`.
#[cfg(test)]
pub(crate) fn gtk_check() {
    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::model::test_support::*;

    let add_to_main_list = |session: &model::ClientStateSession, id: i64| {
        session.handle_update(tdlib::enums::Update::ChatPosition(
            tdlib::types::UpdateChatPosition {
                chat_id: id,
                position: tdlib::types::ChatPosition {
                    list: tdlib::enums::ChatList::Main,
                    order: id,
                    is_pinned: false,
                    source: None,
                },
            },
        ));
    };
    let search = |dialog: &ChatPickerDialog, text: &str| {
        let entry = &dialog.imp().search_entry;
        entry.set_text(text);
        // The entry reports changes with a delay.
        entry.emit_by_name::<()>("search-changed", &[]);
    };
    let n_items = |dialog: &ChatPickerDialog| dialog.imp().list_view.model().unwrap().n_items();

    eprintln!("chat picker: lists and filters the main chat list");
    isolated(|| {
        let session = session();
        add_user(&session, 42);
        for (id, title) in [(1, "Alice"), (2, "Bob"), (3, "Alpine")] {
            add_private_chat(&session, id, 42, title);
            add_to_main_list(&session, id);
        }

        let dialog = ChatPickerDialog::new(&session);
        assert_eq!(n_items(&dialog), 3);
        assert_eq!(
            dialog.imp().stack.visible_child_name().as_deref(),
            Some("list")
        );

        search(&dialog, "al");
        assert_eq!(n_items(&dialog), 2);
        search(&dialog, "zzz");
        assert_eq!(n_items(&dialog), 0);
        assert_eq!(
            dialog.imp().stack.visible_child_name().as_deref(),
            Some("empty")
        );
        search(&dialog, "");
        assert_eq!(n_items(&dialog), 3);
        assert_eq!(
            dialog.imp().stack.visible_child_name().as_deref(),
            Some("list")
        );
    });

    eprintln!("chat picker: activating a row emits chat-picked");
    isolated(|| {
        let session = session();
        add_user(&session, 42);
        for (id, title) in [(1, "Alice"), (2, "Bob")] {
            add_private_chat(&session, id, 42, title);
            add_to_main_list(&session, id);
        }

        let dialog = ChatPickerDialog::new(&session);
        let picked = Rc::new(RefCell::new(Vec::new()));
        dialog.connect_chat_picked({
            let picked = picked.clone();
            move |_, chat| picked.borrow_mut().push(chat.id())
        });

        // The newest chat comes first.
        dialog
            .imp()
            .list_view
            .emit_by_name::<()>("activate", &[&0u32]);
        assert_eq!(*picked.borrow(), [2]);

        // Enter in the search picks the first match.
        search(&dialog, "ali");
        dialog
            .imp()
            .search_entry
            .emit_by_name::<()>("activate", &[]);
        assert_eq!(*picked.borrow(), [2, 1]);
    });
}
