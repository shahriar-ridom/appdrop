use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use std::path::PathBuf;
use std::rc::Rc;

use crate::installer::{self, InstalledApp};

/// Ask for an AppImage with the native file chooser.
pub fn choose_appimage(
    window: &adw::ApplicationWindow,
    title: &str,
    on_choose: impl Fn(PathBuf) + 'static,
) {
    let dialog = gtk::FileChooserNative::builder()
        .title(title)
        .transient_for(window)
        .action(gtk::FileChooserAction::Open)
        .accept_label("Select")
        .cancel_label("Cancel")
        .build();

    let filter = gtk::FileFilter::new();
    filter.set_name(Some("AppImage files"));
    filter.add_pattern("*.AppImage");
    filter.add_pattern("*.appimage");
    dialog.add_filter(&filter);

    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept
            && let Some(path) = dialog.file().and_then(|file| file.path())
        {
            on_choose(path);
        }

        dialog.destroy();
    });

    dialog.show();
}

/// The "what do I have installed" page.
#[derive(Clone)]
pub struct Library {
    pub widget: gtk::Stack,
    list: gtk::ListBox,
    count: gtk::Label,
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    on_update: Rc<dyn Fn(PathBuf)>,
}

impl Library {
    /// `on_install` is called when the user wants to install something;
    /// `on_update` with an AppImage picked to update an installed app.
    pub fn new(
        window: &adw::ApplicationWindow,
        toasts: &adw::ToastOverlay,
        on_install: Rc<dyn Fn()>,
        on_update: Rc<dyn Fn(PathBuf)>,
    ) -> Library {
        let count = gtk::Label::new(None);
        count.add_css_class("appdrop-section-title");
        count.set_halign(gtk::Align::Start);

        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);

        let install_button = gtk::Button::with_label("Install an AppImage");
        install_button.add_css_class("appdrop-primary");
        let on_install_list = on_install.clone();
        install_button.connect_clicked(move |_| on_install_list());

        let content = gtk::Box::new(gtk::Orientation::Vertical, 14);
        content.set_margin_top(12);
        content.set_margin_bottom(26);
        content.set_margin_start(24);
        content.set_margin_end(24);
        content.append(&count);
        content.append(&list);
        content.append(&install_button);

        let clamp = adw::Clamp::builder()
            .maximum_size(600)
            .child(&content)
            .build();

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();

        let empty_button = gtk::Button::with_label("Install an AppImage");
        empty_button.add_css_class("appdrop-primary");
        empty_button.add_css_class("pill");
        empty_button.set_halign(gtk::Align::Center);
        let on_install_empty = on_install.clone();
        empty_button.connect_clicked(move |_| on_install_empty());

        let empty = adw::StatusPage::builder()
            .icon_name("folder-symbolic")
            .title("No apps installed yet")
            .description("Apps you install with AppDrop will show up here.")
            .child(&empty_button)
            .build();

        let widget = gtk::Stack::new();
        widget.add_named(&scrolled, Some("list"));
        widget.add_named(&empty, Some("empty"));

        Library {
            widget,
            list,
            count,
            window: window.clone(),
            toasts: toasts.clone(),
            on_update,
        }
    }

    pub fn refresh(&self) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }

        let apps = installer::list_installed();

        for app in &apps {
            self.list.append(&self.row(app));
        }

        self.count
            .set_text(&format!("INSTALLED APPS · {}", apps.len()));
        self.widget
            .set_visible_child_name(if apps.is_empty() { "empty" } else { "list" });
    }

    fn toast(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::new(message));
    }

    fn row(&self, app: &InstalledApp) -> adw::ActionRow {
        let mut details = Vec::new();
        if let Some(version) = &app.version {
            details.push(format!("Version {version}"));
        }
        details.push(gtk::glib::format_size(app.size).to_string());

        let row = adw::ActionRow::builder()
            .title(&app.name)
            .subtitle(details.join(" · "))
            .use_markup(false)
            .build();

        let icon = match &app.icon {
            Some(path) => gtk::Image::from_file(path),
            None => gtk::Image::from_icon_name("application-x-executable-symbolic"),
        };
        icon.set_pixel_size(40);
        row.add_prefix(&icon);

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        buttons.set_valign(gtk::Align::Center);

        let action = |icon: &str, tooltip: &str| {
            let button = gtk::Button::from_icon_name(icon);
            button.add_css_class("flat");
            button.set_tooltip_text(Some(tooltip));
            button.update_property(&[gtk::accessible::Property::Label(tooltip)]);
            button
        };

        let launch = action("media-playback-start-symbolic", "Launch");
        let library = self.clone();
        let launched = app.clone();
        launch.connect_clicked(move |_| {
            if let Err(error) = installer::launch(&launched) {
                library.toast(&format!("Could not launch {}: {error}", launched.name));
            }
        });

        let update = action(
            "software-update-available-symbolic",
            "Update from a newer AppImage…",
        );
        let library = self.clone();
        let updating = app.name.clone();
        update.connect_clicked(move |_| {
            let on_update = library.on_update.clone();
            choose_appimage(
                &library.window,
                &format!("Choose a different version of {updating}"),
                move |path| on_update(path),
            );
        });

        let remove = action("user-trash-symbolic", "Uninstall");
        let library = self.clone();
        let removing = app.clone();
        remove.connect_clicked(move |_| library.confirm_uninstall(removing.clone()));

        buttons.append(&launch);
        buttons.append(&update);
        buttons.append(&remove);
        row.add_suffix(&buttons);

        row
    }

    fn confirm_uninstall(&self, app: InstalledApp) {
        let dialog = adw::AlertDialog::new(
            Some(&format!("Uninstall {}?", app.name)),
            Some(
                "The app and its launcher will be removed. The AppImage you downloaded is not touched.",
            ),
        );
        dialog.add_responses(&[("cancel", "Cancel"), ("uninstall", "Uninstall")]);
        dialog.set_response_appearance("uninstall", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let library = self.clone();

        dialog.connect_response(None, move |_, response| {
            if response != "uninstall" {
                return;
            }

            match installer::uninstall_id(&app.id, &app.name) {
                Ok(()) => library.toast(&format!("{} has been uninstalled.", app.name)),
                Err(error) => library.toast(&format!("Uninstall failed: {error}")),
            }

            library.refresh();
        });

        dialog.present(Some(&self.window));
    }
}
