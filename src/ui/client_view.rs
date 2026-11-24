use glib::closure;
use glib::subclass::InitializingObject;
use glib::Properties;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::CompositeTemplate;

use crate::model;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate, Properties)]
    #[properties(wrapper_type = super::ClientView)]
    #[template(resource = "/app/drey/paper-plane/ui/client_view.ui")]
    pub(crate) struct ClientView {
        #[property(get, set)]
        pub(super) model: glib::WeakRef<model::Client>,
        #[template_child]
        pub(super) bin: TemplateChild<adw::Bin>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ClientView {
        const NAME: &'static str = "PaplClientView";
        type Type = super::ClientView;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ClientView {
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
            self.obj().setup_expressions();
        }

        fn dispose(&self) {
            utils::unparent_children(&*self.obj());
        }
    }

    impl WidgetImpl for ClientView {}
}

glib::wrapper! {
    pub(crate) struct ClientView(ObjectSubclass<imp::ClientView>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl From<&crate::model::Client> for ClientView {
    fn from(model: &crate::model::Client) -> Self {
        glib::Object::builder().property("model", model).build()
    }
}

impl ClientView {
    fn setup_expressions(&self) {
        Self::this_expression("model")
            .chain_property::<crate::model::Client>("state")
            .chain_closure::<gtk::Widget>(closure!(|_: Self, state: glib::Object| {
                if let Some(state) = state.downcast_ref::<model::ClientStateAuth>() {
                    ui::Login::from(state).upcast::<gtk::Widget>()
                } else if let Some(state) = state.downcast_ref::<model::ClientStateSession>() {
                    ui::Session::from(state).upcast::<gtk::Widget>()
                } else {
                    // Logging out or closing: nothing to interact with any more
                    adw::Spinner::new().upcast::<gtk::Widget>()
                }
            }))
            .bind(&*self.imp().bin, "child", Some(self));
    }
}

/// Checks of the client view that need GTK, see `ui::tests`.
#[cfg(test)]
pub(crate) fn gtk_check() {
    use adw::prelude::*;

    use crate::model::test_support::*;

    eprintln!("client view while logging out and closing");

    isolated(|| {
        let client = client();
        let view = ClientView::from(&client);
        assert!(view.imp().bin.child().and_downcast::<ui::Login>().is_some());

        // TDLib closing the client (log out, quit) used to panic here
        // (vitlav e37f1b81); the view shows a spinner instead.
        client.handle_update(tdlib::enums::Update::AuthorizationState(
            tdlib::types::UpdateAuthorizationState {
                authorization_state: tdlib::enums::AuthorizationState::Closing,
            },
        ));
        assert!(client
            .state()
            .and_downcast::<model::ClientStateLoggingOut>()
            .is_some());
        assert!(view
            .imp()
            .bin
            .child()
            .and_downcast::<adw::Spinner>()
            .is_some());
    });
}
