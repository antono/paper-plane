use gtk::glib;
use gtk::glib::clone;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct ReactionsChooser {
        pub(super) message: glib::WeakRef<model::Message>,
        pub(super) flow_box: gtk::FlowBox,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ReactionsChooser {
        const NAME: &'static str = "PaplReactionsChooser";
        type Type = super::ReactionsChooser;
        type ParentType = gtk::Popover;

        fn new() -> Self {
            let flow_box = gtk::FlowBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .homogeneous(true)
                .max_children_per_line(7)
                .build();

            Self {
                message: glib::WeakRef::new(),
                flow_box,
            }
        }

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("reactionschooser");
        }
    }

    impl ObjectImpl for ReactionsChooser {
        fn constructed(&self) {
            self.parent_constructed();

            let scrolled_window = gtk::ScrolledWindow::builder()
                .propagate_natural_height(true)
                .propagate_natural_width(true)
                .max_content_height(400)
                .child(&self.flow_box)
                .build();

            self.obj().set_child(Some(&scrolled_window));
        }
    }

    impl PopoverImpl for ReactionsChooser {}
    impl WidgetImpl for ReactionsChooser {}
}

glib::wrapper! {
    pub(crate) struct ReactionsChooser(ObjectSubclass<imp::ReactionsChooser>)
        @extends gtk::Popover, gtk::Widget;
}

impl ReactionsChooser {
    pub(crate) fn new(message: &model::Message) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.imp().message.set(Some(message));
        obj
    }

    pub(crate) fn set_reactions(&self, reactions: Vec<tdlib::types::AvailableReaction>) {
        let imp = self.imp();

        // Through the flow box: unparenting its children directly leaves
        // them in its internal list, and it then shows no new buttons.
        imp.flow_box.remove_all();

        for reaction in reactions {
            let emoji = match &reaction.r#type {
                tdlib::enums::ReactionType::Emoji(data) => data.emoji.clone(),
                // TODO: Support custom emoji reactions
                tdlib::enums::ReactionType::CustomEmoji(_) => continue,
                tdlib::enums::ReactionType::Paid => continue,
            };

            let label = gtk::Label::builder()
                .label(utils::emoji_string(&emoji))
                .css_classes(["emoji"])
                .build();

            let button = gtk::Button::builder()
                .child(&label)
                .css_classes(["emoji"])
                .build();

            if reaction.needs_premium {
                button.add_css_class("premium");
            }

            button.connect_clicked(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_| {
                    obj.popdown();
                    if let Some(message) = obj.imp().message.upgrade() {
                        message.toggle_reaction(&emoji);
                    }
                }
            ));

            imp.flow_box.append(&button);
        }
    }
}

/// Checks of the reaction picker that need GTK, see `ui::tests`.
#[cfg(test)]
pub(crate) fn gtk_check() {
    use crate::model::test_support::*;

    eprintln!("reaction picker");

    isolated(|| {
        let session = session();
        let chat = add_chat(&session, ME);
        let message = model::Message::new(&chat, message(ME, 1, DAY, 0, text("react")));

        let reaction = |r#type, needs_premium| tdlib::types::AvailableReaction {
            r#type,
            needs_premium,
        };
        let emoji = |emoji: &str| {
            tdlib::enums::ReactionType::Emoji(tdlib::types::ReactionTypeEmoji {
                emoji: emoji.to_owned(),
            })
        };

        let chooser = ReactionsChooser::new(&message);
        chooser.set_reactions(vec![
            reaction(emoji("👍"), false),
            reaction(
                tdlib::enums::ReactionType::CustomEmoji(tdlib::types::ReactionTypeCustomEmoji {
                    custom_emoji_id: 1,
                }),
                false,
            ),
            reaction(tdlib::enums::ReactionType::Paid, false),
            reaction(emoji("🔥"), true),
        ]);

        let buttons: Vec<gtk::Button> = chooser
            .imp()
            .flow_box
            .observe_children()
            .snapshot()
            .into_iter()
            .filter_map(|child| {
                child
                    .downcast::<gtk::Widget>()
                    .ok()
                    .and_then(|w| w.first_child().or(Some(w)))
                    .and_then(|w| w.downcast::<gtk::Button>().ok())
            })
            .collect();
        // Custom emoji and paid reactions are not offered yet.
        assert_eq!(buttons.len(), 2);
        assert!(!buttons[0].has_css_class("premium"));
        assert!(buttons[1].has_css_class("premium"));

        // Replacing the reactions replaces the buttons.
        chooser.set_reactions(vec![reaction(emoji("❤️"), false)]);
        assert_eq!(chooser.imp().flow_box.observe_children().n_items(), 1);
    });
}
