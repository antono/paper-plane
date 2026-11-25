//! The row of reactions at the top of the message menu, as in Telegram for
//! phones: the first few reactions, and a button that shows all of them.

use std::cell::Cell;
use std::cell::RefCell;

use gettextrs::gettext;
use glib::clone;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::ui;
use crate::utils;

/// How many reactions the row shows before the button that shows all. Seven
/// buttons fit into the menu on a 360 pixels wide phone.
pub(crate) const QUICK_COUNT: usize = 6;
/// The size of the picture of a custom emoji in the row.
const ICON_SIZE: i32 = 28;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct QuickReactions {
        pub(super) message: glib::WeakRef<model::Message>,
        /// Counts the messages shown, so that the reactions of a message
        /// arriving late do not replace those of the next one.
        pub(super) generation: Cell<u32>,
        pub(super) reactions: RefCell<Vec<tdlib::types::AvailableReaction>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for QuickReactions {
        const NAME: &'static str = "PaplQuickReactions";
        type Type = super::QuickReactions;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("quickreactions");
        }
    }

    impl ObjectImpl for QuickReactions {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_spacing(2);
            obj.set_halign(gtk::Align::Center);
            obj.set_visible(false);
        }
    }

    impl WidgetImpl for QuickReactions {}
    impl BoxImpl for QuickReactions {}
}

glib::wrapper! {
    pub(crate) struct QuickReactions(ObjectSubclass<imp::QuickReactions>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Default for QuickReactions {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl QuickReactions {
    /// The message whose reactions are shown.
    #[cfg(test)]
    pub(crate) fn message(&self) -> Option<model::Message> {
        self.imp().message.upgrade()
    }

    /// Shows the reactions of `message`, fetched from TDLib. The row is
    /// hidden without a message, or if its chat allows no reactions.
    pub(crate) fn set_message(&self, message: Option<&model::Message>) {
        let imp = self.imp();
        let generation = imp.generation.get().wrapping_add(1);
        imp.generation.set(generation);
        imp.message.set(message);
        self.set_reactions(Vec::new());

        let Some(message) = message.filter(|message| message.chat_().can_react()) else {
            return;
        };

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            #[strong]
            message,
            async move {
                match message.available_reactions().await {
                    Ok(reactions) if obj.imp().generation.get() == generation => {
                        obj.set_reactions(reactions);
                    }
                    Ok(_) => {}
                    Err(e) => log::debug!("Error getting available message reactions: {e:?}"),
                }
            }
        ));
    }

    /// Replaces the buttons. Paid reactions are not offered.
    pub(crate) fn set_reactions(&self, reactions: Vec<tdlib::types::AvailableReaction>) {
        let imp = self.imp();
        utils::unparent_children(self);

        let reactions: Vec<_> = reactions
            .into_iter()
            .filter(|reaction| !matches!(reaction.r#type, tdlib::enums::ReactionType::Paid))
            .collect();
        let session = imp
            .message
            .upgrade()
            .map(|message| message.chat_().session_());

        for reaction in reactions.iter().take(QUICK_COUNT) {
            let button = gtk::Button::builder()
                .child(&super::reaction_icon::new(
                    &reaction.r#type,
                    session.clone(),
                    ICON_SIZE,
                ))
                .css_classes(["flat", "circular", "quick-reaction"])
                .build();
            if reaction.needs_premium {
                button.add_css_class("premium");
            }
            let reaction_type = reaction.r#type.clone();
            button.connect_clicked(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_| obj.react(reaction_type.clone())
            ));
            self.append(&button);
        }

        if reactions.len() > QUICK_COUNT {
            let expand = gtk::Button::builder()
                .icon_name("pan-down-symbolic")
                .tooltip_text(gettext("All Reactions"))
                .css_classes(["flat", "circular", "expand"])
                .build();
            expand.connect_clicked(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_| obj.show_all()
            ));
            self.append(&expand);
        }

        self.set_visible(!reactions.is_empty());
        imp.reactions.replace(reactions);
    }

    fn menu(&self) -> Option<gtk::Popover> {
        self.ancestor(gtk::Popover::static_type()).and_downcast()
    }

    fn react(&self, reaction_type: tdlib::enums::ReactionType) {
        if let Some(menu) = self.menu() {
            menu.popdown();
        }
        if let Some(message) = self.imp().message.upgrade() {
            message.toggle_reaction(reaction_type);
        }
    }

    /// Replaces the menu by the picker of all reactions, at the same place.
    fn show_all(&self) {
        let imp = self.imp();
        let Some(message) = imp.message.upgrade() else {
            return;
        };
        let Some(menu) = self.menu() else {
            return;
        };
        let Some(parent) = menu.parent() else {
            return;
        };
        let pointing_to = menu.pointing_to();
        menu.popdown();

        let chooser = ui::ReactionsChooser::new(&message);
        chooser.set_reactions(imp.reactions.borrow().clone());
        chooser.set_parent(&parent);
        if let (true, rectangle) = pointing_to {
            chooser.set_pointing_to(Some(&rectangle));
        }
        chooser.connect_closed(|chooser| chooser.unparent());
        chooser.popup();
    }
}

/// Checks of the reaction row that need GTK, see `ui::tests`.
#[cfg(test)]
pub(crate) fn gtk_check() {
    use crate::model::test_support::*;

    eprintln!("quick reactions of the message menu");

    isolated(|| {
        let session = session();
        let chat = add_chat(&session, ME);
        let message = model::Message::new(&chat, message(ME, 1, DAY, 0, text("react")));

        let available = |keys: &[&str]| -> Vec<tdlib::types::AvailableReaction> {
            keys.iter()
                .map(|key| tdlib::types::AvailableReaction {
                    r#type: model::reaction_type_from_key(key),
                    needs_premium: false,
                })
                .collect()
        };
        let buttons = |row: &QuickReactions| -> Vec<gtk::Button> {
            row.observe_children()
                .snapshot()
                .into_iter()
                .filter_map(|child| child.downcast::<gtk::Button>().ok())
                .collect()
        };

        let row = QuickReactions::default();
        // A message of a chat without reactions hides the row.
        row.set_message(None);
        assert!(!row.is_visible());

        row.imp().message.set(Some(&message));
        row.set_reactions(available(&["👍", "⭐", "❤", "[ce137]"]));
        assert!(row.is_visible());
        let shown = buttons(&row);
        // Paid reactions are not offered, and all fit without expanding.
        assert_eq!(shown.len(), 3);
        assert!(shown.iter().all(|b| b.has_css_class("quick-reaction")));

        // More than fit get the button that shows all of them.
        row.set_reactions(available(&["1️⃣", "2️⃣", "3️⃣", "4️⃣", "5️⃣", "6️⃣", "7️⃣"]));
        let shown = buttons(&row);
        assert_eq!(shown.len(), QUICK_COUNT + 1);
        assert!(shown.last().unwrap().has_css_class("expand"));
        assert_eq!(row.imp().reactions.borrow().len(), 7);

        // The expand button opens the picker with all of them at the place
        // of the menu.
        let anchor = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let window = gtk::Window::new();
        window.set_child(Some(&anchor));
        let menu = gtk::Popover::new();
        menu.set_child(Some(&row));
        menu.set_parent(&anchor);
        menu.set_pointing_to(Some(&gtk::gdk::Rectangle::new(13, 7, 0, 0)));
        shown.last().unwrap().emit_clicked();
        let chooser = anchor
            .observe_children()
            .snapshot()
            .into_iter()
            .find_map(|child| child.downcast::<ui::ReactionsChooser>().ok())
            .expect("the picker of all reactions");
        assert_eq!(chooser.pointing_to(), menu.pointing_to());
        assert_eq!((chooser.pointing_to().1.x(), chooser.pointing_to().1.y()), (13, 7));
        chooser.unparent();
        menu.unparent();
        window.destroy();

        // A message without reactions in its chat hides the row again.
        row.set_message(None);
        assert!(!row.is_visible());
        assert!(buttons(&row).is_empty());
    });
}
