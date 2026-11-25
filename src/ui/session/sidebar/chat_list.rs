use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;

use glib::clone;
use glib::Properties;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::CompositeTemplate;

use crate::model;
use crate::ui;
use crate::utils;

/// The most rows a chat list keeps for reuse. A list view shows at most
/// 200 rows, see `GTK_LIST_VIEW_MAX_LIST_ITEMS`.
const ROW_POOL_CAPACITY: usize = 256;

mod imp {
    use super::*;

    #[derive(Debug, Default, Properties, CompositeTemplate)]
    #[properties(wrapper_type = super::ChatList)]
    #[template(resource = "/app/drey/paper-plane/ui/session/sidebar/chat_list.ui")]
    pub(crate) struct ChatList {
        pub(super) marked_as_unread_handler_id: RefCell<Option<glib::SignalHandlerId>>,
        #[property(get, set = Self::set_selected_chat, nullable, explicit_notify)]
        pub(super) selected_chat: glib::WeakRef<model::Chat>,
        #[property(get, set)]
        pub(super) chat_list: glib::WeakRef<model::ChatList>,
        #[property(get, set)]
        pub(super) compact: Cell<bool>,
        #[template_child]
        pub(super) selection: TemplateChild<ui::SidebarSelection>,
        #[template_child]
        pub(super) scrolled_window: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub(super) list_view: TemplateChild<gtk::ListView>,
        /// The rows that the list view let go of, see `ChatList::setup_factory`.
        pub(super) row_pool: RefCell<Vec<ui::SidebarRow>>,
        /// The rows of the list items of the list view. GTK takes the child
        /// of a list item before tearing it down.
        pub(super) rows: RefCell<HashMap<gtk::ListItem, ui::SidebarRow>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatList {
        const NAME: &'static str = "PaplSidebarChatList";
        type Type = super::ChatList;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
            klass.set_css_name("chatlist");
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ChatList {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            self.derived_set_property(id, value, pspec)
        }

        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            self.derived_property(id, pspec)
        }

        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup_factory();
        }

        fn dispose(&self) {
            utils::unparent_children(&*self.obj());
            self.rows.take();
            self.row_pool.take();
        }
    }

    impl WidgetImpl for ChatList {}

    #[gtk::template_callbacks]
    impl ChatList {
        pub(crate) fn set_selected_chat(&self, selected_chat: Option<&model::Chat>) {
            let obj = &*self.obj();
            if obj.selected_chat().as_ref() == selected_chat {
                return;
            }

            if let Some(handler_id) = self.marked_as_unread_handler_id.take() {
                obj.selected_chat().unwrap().disconnect(handler_id);
            }

            match selected_chat {
                Some(chat) => {
                    let handler_id = chat.connect_notify_local(
                        Some("is-marked-as-unread"),
                        clone!(
                            #[weak(rename_to = obj)]
                            self,
                            move |chat, _| {
                                if chat.is_marked_as_unread() {
                                    obj.set_selected_chat(None);
                                }
                            }
                        ),
                    );
                    self.marked_as_unread_handler_id.replace(Some(handler_id));

                    self.selection.set_selected_chat(Some(chat));

                    if chat.is_marked_as_unread() {
                        utils::spawn(clone!(
                            #[weak]
                            chat,
                            async move {
                                if let Err(e) = chat.mark_as_read().await {
                                    log::warn!("Error on toggling chat's unread state: {e:?}");
                                }
                            }
                        ));
                    }
                }
                None => self.selection.set_selected_chat(None),
            }

            self.selected_chat.set(selected_chat);

            obj.notify_selected_chat();
        }

        #[template_callback]
        fn on_notify_chat_list(&self) {
            self.selection
                .set_selected_chat(self.obj().selected_chat().as_ref());
        }

        #[template_callback]
        fn list_activate(&self, pos: u32) {
            let obj = &*self.obj();
            self.selection.set_selected_position(pos);

            obj.set_selected_chat(self.selection.selected_chat());
            obj.activate_action("navigation.push", Some(&"content".to_variant()))
                .unwrap();
        }
    }
}

glib::wrapper! {
    pub(crate) struct ChatList(ObjectSubclass<imp::ChatList>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ChatList {
    /// Sets up the factory of the rows of the list view.
    ///
    /// Building a row from its template is the most expensive part of
    /// showing a chat, and switching the chat folder replaces every item of
    /// the list, for which the list view drops all of its rows and makes new
    /// ones. So the rows it drops are kept in a pool, and new rows are taken
    /// from it. As the list view makes the new rows before dropping the old
    /// ones, the selection reports a new chat list as a removal of all the
    /// rows followed by an addition, see `SidebarSelection::set_model`.
    fn setup_factory(&self) {
        let factory = gtk::SignalListItemFactory::new();

        factory.connect_setup(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, list_item| {
                let imp = obj.imp();
                let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
                let row = imp.row_pool.borrow_mut().pop().unwrap_or_default();
                list_item.set_child(Some(&row));
                imp.rows.borrow_mut().insert(list_item.clone(), row);
            }
        ));
        factory.connect_bind(|_, list_item| {
            let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(row) = list_item.child().and_downcast::<ui::SidebarRow>() {
                row.set_item(list_item.item().and_downcast_ref());
            }
        });
        // An unbound row keeps its item until it is bound to another one or
        // torn down: emptying it would update it twice when it is recycled.
        factory.connect_teardown(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, list_item| {
                let imp = obj.imp();
                let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
                let Some(row) = imp.rows.borrow_mut().remove(list_item) else {
                    return;
                };
                if list_item.child().is_some() {
                    list_item.set_child(None::<&gtk::Widget>);
                }
                if row.parent().is_some() {
                    row.unparent();
                }
                row.set_item(None);

                let mut pool = imp.row_pool.borrow_mut();
                if pool.len() < ROW_POOL_CAPACITY {
                    pool.push(row);
                }
            }
        ));

        self.imp().list_view.set_factory(Some(&factory));
    }
}

#[cfg(test)]
impl ChatList {
    fn new_for_test() -> Self {
        glib::Object::new()
    }
}

/// The list view scrolls itself, see `ui::tests`.
///
/// Inside a GtkScrolledWindow the chat list (not scrollable) got a
/// GtkViewport, so the GtkListView had its own adjustment and did not know
/// which rows were on screen: on a touch it dropped the widgets of rows that
/// were still shown, leaving holes (seen on the phone).
#[cfg(test)]
pub(crate) fn gtk_check() {
    use crate::model::test_support::*;

    eprintln!("chat list scrolls its list view directly");

    isolated(|| {
        let list = ChatList::new_for_test();
        let imp = list.imp();

        assert_eq!(
            imp.list_view.parent().as_ref(),
            Some(imp.scrolled_window.upcast_ref::<gtk::Widget>()),
            "the list view must be the direct child of the scrolled window"
        );
        assert_eq!(
            imp.list_view.vadjustment().as_ref(),
            Some(&imp.scrolled_window.vadjustment()),
            "the list view must scroll with the scrolled window"
        );

        // The sidebar must not wrap the chat list into another scrolled window
        let sidebar = include_str!("mod.ui");
        for part in sidebar.split(r#"class="GtkScrolledWindow""#).skip(1) {
            let object = part.split("</object>").next().unwrap_or_default();
            assert!(
                !object.contains("PaplSidebarChatList"),
                "a PaplSidebarChatList inside a GtkScrolledWindow in mod.ui"
            );
        }
    });
}

/// The rows are reused across chat lists, see `ui::tests`.
#[cfg(test)]
pub(crate) fn gtk_check_row_pool() {
    use gtk::gio;

    use crate::model::test_support::*;

    eprintln!("chat list reuses its rows for another chat list");

    isolated(|| {
        let session = session();
        let position = tdlib::types::ChatPosition {
            list: tdlib::enums::ChatList::Main,
            order: 1,
            is_pinned: false,
            source: None,
        };
        let chats: Vec<_> = (1..=5)
            .map(|id| {
                add_user(&session, id);
                add_private_chat(&session, id, id, &format!("Chat {id}"))
            })
            .collect();
        // The same chats in two chat lists are different items.
        let chat_list = || {
            let store = gio::ListStore::new::<model::ChatListItem>();
            for chat in &chats {
                store.append(&model::ChatListItem::new(chat, &position));
            }
            store.upcast::<gio::ListModel>()
        };

        let list = ChatList::new_for_test();
        let window = gtk::Window::builder()
            .default_width(360)
            .default_height(600)
            .child(&list)
            .build();
        window.present();

        let context = glib::MainContext::default();
        let rows = || {
            let mut rows = Vec::new();
            let mut child = list.imp().list_view.first_child();
            while let Some(widget) = child {
                if let Some(row) = widget.first_child().and_downcast::<ui::SidebarRow>() {
                    rows.push(row);
                }
                child = widget.next_sibling();
            }
            rows
        };
        let settle = |n_rows: usize| {
            for _ in 0..200 {
                while context.iteration(false) {}
                if rows().len() == n_rows {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            panic!("{} rows instead of {n_rows}", rows().len());
        };

        list.imp().selection.set_model(Some(chat_list()));
        settle(chats.len());
        let before = rows();

        list.imp().selection.set_model(Some(chat_list()));
        settle(chats.len());
        let after = rows();

        for row in &after {
            assert!(before.contains(row), "a new row instead of a pooled one");
            assert!(row.item().is_some(), "a row without its item");
        }
        assert!(list.imp().row_pool.borrow().is_empty());

        // The rows of an emptied list wait in the pool, without their items.
        list.imp().selection.set_model(None);
        settle(0);
        let pool = list.imp().row_pool.borrow().clone();
        assert_eq!(pool.len(), chats.len());
        assert!(pool
            .iter()
            .all(|row| row.item().is_none() && row.parent().is_none()));
        assert!(list.imp().rows.borrow().is_empty());
        // A pooled row still has the widgets of its template.
        assert!(pool.iter().all(|row| row.first_child().is_some()));

        window.destroy();
    });
}
