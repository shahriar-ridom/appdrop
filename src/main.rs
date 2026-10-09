mod appimage;
mod installer;
mod metadata;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

type SelectedPath = Rc<RefCell<Option<PathBuf>>>;

fn main() {
    let app = adw::Application::builder()
        .application_id("dev.appdrop.Installer")
        .flags(gtk::gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    app.connect_activate(|app| {
        if app.active_window().is_none() {
            build_ui(app, None);
        }
    });

    app.connect_open(|app, files, _hint| {
        if let Some(path) = files.first().and_then(|file| file.path()) {
            build_ui(app, Some(path));
        }
    });

    app.run();
}

fn set_installed_view(
    installed: bool,
    arrow: &gtk::Image,
    folder_card: &gtk::Frame,
    choose_button: &gtk::Button,
    uninstall_button: &gtk::Button,
    drag_source: &gtk::DragSource,
    app_hint: &gtk::Label,
) {
    arrow.set_visible(!installed);
    folder_card.set_visible(!installed);
    choose_button.set_visible(!installed);

    uninstall_button.set_visible(installed);
    uninstall_button.set_sensitive(installed);

    drag_source.set_actions(if installed {
        gtk::gdk::DragAction::empty()
    } else {
        gtk::gdk::DragAction::COPY
    });

    app_hint.set_text(if installed {
        "Installed application"
    } else {
        "Drag this icon to install"
    });
}

fn inspect_into_ui(
    path: PathBuf,
    app_icon: &gtk::Image,
    app_name: &gtk::Label,
    app_details: &gtk::Label,
    status: &gtk::Label,
    arrow: &gtk::Image,
    folder_card: &gtk::Frame,
    choose_button: &gtk::Button,
    uninstall_button: &gtk::Button,
    drag_source: &gtk::DragSource,
    app_hint: &gtk::Label,
    selected_path: &SelectedPath,
) {
    status.set_text("Inspecting application…");

    match metadata::inspect(&path) {
        Ok(info) => {
            let icon_name = info
                .icon
                .as_deref()
                .filter(|icon| !icon.is_empty() && !icon.contains('/'))
                .unwrap_or("application-x-executable-symbolic");

            app_icon.set_icon_name(Some(icon_name));
            app_name.set_text(&info.name);

            let categories = if info.categories.is_empty() {
                "Not specified".to_string()
            } else {
                info.categories.join(", ")
            };

            app_details.set_text(&format!(
                "Desktop entry: {}\n\
                 Icon: {}\n\
                 Categories: {}\n\
                 Launch command: {}\n\
                 SquashFS offset: {} bytes",
                info.desktop_file,
                info.icon.as_deref().unwrap_or("Not specified"),
                categories,
                info.exec.as_deref().unwrap_or("Not specified"),
                info.squashfs_offset,
            ));

            let installed = installer::is_installed(&info);

            set_installed_view(
                installed,
                arrow,
                folder_card,
                choose_button,
                uninstall_button,
                drag_source,
                app_hint,
            );

            status.set_text(if installed {
                "This application is managed by AppDrop."
            } else {
                "Ready to install. Drag the app icon into Applications."
            });

            *selected_path.borrow_mut() = Some(path);
        }
        Err(error) => {
            app_icon.set_icon_name(Some("application-x-executable-symbolic"));
            app_name.set_text("Unsupported application");
            app_details.set_text("AppDrop couldn't read this application's metadata.");

            status.set_text(&format!("Inspection failed: {error}"));

            set_installed_view(
                false,
                arrow,
                folder_card,
                choose_button,
                uninstall_button,
                drag_source,
                app_hint,
            );

            *selected_path.borrow_mut() = None;
        }
    }
}

fn build_ui(app: &adw::Application, initial_path: Option<PathBuf>) {
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::PreferDark);

    // AppDrop's theme

    let css_provider = gtk::CssProvider::new();

    css_provider.load_from_data(
        r#"
        .appdrop-window {
            background-color: #111116;
            color: #f4f1fa;
        }

        .appdrop-heading {
            color: #f7f4ff;
            font-size: 30px;
            font-weight: 800;
            letter-spacing: -0.8px;
        }

        .appdrop-subtitle {
            color: #aaa5b8;
            font-size: 14px;
        }

        .appdrop-eyebrow {
            color: #b49aff;
            font-size: 10px;
            font-weight: 800;
            letter-spacing: 1.5px;
        }

        .appdrop-card {
            background-color: #1b1a23;
            color: #f4f1fa;
            border: 1px solid #34313f;
            border-radius: 18px;
        }

        .appdrop-icon-tile {
            background-color: #29213c;
            border: 1px solid #403258;
            border-radius: 20px;
            padding: 18px;
        }

        .appdrop-app-name {
            color: #f7f4ff;
            font-size: 19px;
            font-weight: 750;
        }

        .appdrop-muted {
            color: #b9b3c8;
        }

        .appdrop-section-title {
            color: #b49aff;
            font-size: 11px;
            font-weight: 750;
            letter-spacing: 0.8px;
        }

        .appdrop-drop-hover {
            background-color: #29213c;
            border: 2px solid #b49aff;
            border-radius: 18px;
        }

        button.appdrop-primary {
            background-color: #8b5cf6;
            color: #ffffff;
            min-height: 44px;
            font-weight: 700;
            border-radius: 12px;
            border: 1px solid #a78bfa;
        }

        button.appdrop-primary:hover {
            background-color: #a78bfa;
            color: #ffffff;
        }

        button.appdrop-secondary {
            background-color: #29213c;
            color: #e2d6ff;
            min-height: 42px;
            border: 1px solid #49395f;
            border-radius: 12px;
        }

        button.appdrop-secondary:hover {
            background-color: #38294e;
            color: #ffffff;
        }

        .appdrop-status {
            color: #b9b3c8;
            font-size: 12px;
        }
        "#,
    );

    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css_provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    // Window.

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("AppDrop")
        .default_width(720)
        .default_height(650)
        .resizable(false)
        .build();

    window.add_css_class("appdrop-window");

    let selected_path: SelectedPath = Rc::new(RefCell::new(None));

    let content = gtk::Box::new(gtk::Orientation::Vertical, 22);
    content.set_margin_top(30);
    content.set_margin_bottom(26);
    content.set_margin_start(32);
    content.set_margin_end(32);

    // Header.

    let brand_icon = gtk::Image::from_icon_name("application-x-executable-symbolic");
    brand_icon.set_pixel_size(24);

    let brand_tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
    brand_tile.add_css_class("appdrop-icon-tile");
    brand_tile.append(&brand_icon);

    let brand_name = gtk::Label::new(Some("AppDrop"));
    brand_name.add_css_class("appdrop-heading");
    brand_name.set_halign(gtk::Align::Start);

    let brand_subtitle = gtk::Label::new(Some("Your apps, beautifully installed."));
    brand_subtitle.add_css_class("appdrop-subtitle");
    brand_subtitle.set_halign(gtk::Align::Start);

    let brand_text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    brand_text.set_valign(gtk::Align::Center);
    brand_text.append(&brand_name);
    brand_text.append(&brand_subtitle);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    header.set_valign(gtk::Align::Center);
    header.append(&brand_tile);
    header.append(&brand_text);

    let eyebrow = gtk::Label::new(Some("APPIMAGE INSTALLER"));
    eyebrow.add_css_class("appdrop-eyebrow");
    eyebrow.set_halign(gtk::Align::Start);

    content.append(&header);
    content.append(&eyebrow);

    // Application card.

    let app_icon = gtk::Image::from_icon_name("application-x-executable-symbolic");
    app_icon.set_pixel_size(76);
    app_icon.set_tooltip_text(Some("Drag to install"));

    let icon_tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
    icon_tile.add_css_class("appdrop-icon-tile");
    icon_tile.set_halign(gtk::Align::Center);
    icon_tile.append(&app_icon);

    let app_name = gtk::Label::new(Some("Your AppImage"));
    app_name.add_css_class("appdrop-app-name");
    app_name.set_wrap(true);
    app_name.set_max_width_chars(24);
    app_name.set_justify(gtk::Justification::Center);

    let app_hint = gtk::Label::new(Some("Drag this icon to install"));
    app_hint.add_css_class("appdrop-muted");
    app_hint.set_justify(gtk::Justification::Center);

    let app_card_content = gtk::Box::new(gtk::Orientation::Vertical, 14);
    app_card_content.set_halign(gtk::Align::Center);
    app_card_content.set_valign(gtk::Align::Center);
    app_card_content.set_margin_top(28);
    app_card_content.set_margin_bottom(28);
    app_card_content.set_margin_start(20);
    app_card_content.set_margin_end(20);

    app_card_content.append(&icon_tile);
    app_card_content.append(&app_name);
    app_card_content.append(&app_hint);

    let app_card = gtk::Frame::new(None);
    app_card.add_css_class("appdrop-card");
    app_card.set_size_request(220, 230);
    app_card.set_child(Some(&app_card_content));

    // Applications destination.

    let arrow = gtk::Image::from_icon_name("arrow-right-symbolic");
    arrow.set_pixel_size(28);
    arrow.add_css_class("appdrop-muted");

    let folder_icon = gtk::Image::from_icon_name("folder-symbolic");
    folder_icon.set_pixel_size(64);

    let folder_name = gtk::Label::new(Some("Applications"));
    folder_name.add_css_class("appdrop-app-name");

    let folder_hint = gtk::Label::new(Some("Drop here to install"));
    folder_hint.add_css_class("appdrop-muted");

    let folder_content = gtk::Box::new(gtk::Orientation::Vertical, 14);
    folder_content.set_halign(gtk::Align::Center);
    folder_content.set_valign(gtk::Align::Center);
    folder_content.set_margin_top(28);
    folder_content.set_margin_bottom(28);
    folder_content.set_margin_start(20);
    folder_content.set_margin_end(20);

    folder_content.append(&folder_icon);
    folder_content.append(&folder_name);
    folder_content.append(&folder_hint);

    let folder_card = gtk::Frame::new(None);
    folder_card.add_css_class("appdrop-card");
    folder_card.set_size_request(220, 230);
    folder_card.set_child(Some(&folder_content));

    let app_row = gtk::Box::new(gtk::Orientation::Horizontal, 20);
    app_row.set_halign(gtk::Align::Center);
    app_row.set_valign(gtk::Align::Center);
    app_row.append(&app_card);
    app_row.append(&arrow);
    app_row.append(&folder_card);

    // Details panel.

    let details_heading = gtk::Label::new(Some("APPLICATION DETAILS"));
    details_heading.add_css_class("appdrop-section-title");
    details_heading.set_halign(gtk::Align::Start);

    let app_details = gtk::Label::new(Some("Choose an AppImage to see its details."));
    app_details.add_css_class("appdrop-muted");
    app_details.set_halign(gtk::Align::Start);
    app_details.set_wrap(true);
    app_details.set_selectable(true);
    app_details.set_xalign(0.0);

    let details_content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    details_content.set_margin_top(18);
    details_content.set_margin_bottom(18);
    details_content.set_margin_start(20);
    details_content.set_margin_end(20);
    details_content.append(&details_heading);
    details_content.append(&app_details);

    let details_card = gtk::Frame::new(None);
    details_card.add_css_class("appdrop-card");
    details_card.set_child(Some(&details_content));

    // Actions.

    let choose_button = gtk::Button::with_label("Choose AppImage");
    choose_button.add_css_class("appdrop-primary");
    choose_button.set_halign(gtk::Align::Fill);

    let uninstall_button = gtk::Button::with_label("Uninstall Application");
    uninstall_button.add_css_class("appdrop-secondary");
    uninstall_button.set_halign(gtk::Align::Fill);
    uninstall_button.set_sensitive(false);
    uninstall_button.set_visible(false);

    // Status row.
    let status_icon = gtk::Image::from_icon_name("emblem-information-symbolic");
    status_icon.set_pixel_size(16);

    let status = gtk::Label::new(Some("Ready"));
    status.add_css_class("appdrop-status");
    status.set_halign(gtk::Align::Start);
    status.set_wrap(true);
    status.set_selectable(true);

    let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    status_row.set_valign(gtk::Align::Center);
    status_row.append(&status_icon);
    status_row.append(&status);

    content.append(&app_row);
    content.append(&details_card);
    content.append(&choose_button);
    content.append(&uninstall_button);
    content.append(&status_row);

    window.set_content(Some(&content));

    // Drag source and floating preview.

    let drag_source = gtk::DragSource::new();
    drag_source.set_actions(gtk::gdk::DragAction::COPY);

    if let Some(display) = gtk::gdk::Display::default() {
        let icon_theme = gtk::IconTheme::for_display(&display);

        let drag_icon = icon_theme.lookup_icon(
            "application-x-executable-symbolic",
            &[],
            64,
            1,
            gtk::TextDirection::None,
            gtk::IconLookupFlags::empty(),
        );

        drag_source.connect_drag_begin(move |source, _drag| {
            source.set_icon(Some(&drag_icon), 32, 32);
        });
    }

    drag_source.connect_prepare(|_, _, _| {
        let value = "appdrop-install".to_value();
        Some(gtk::gdk::ContentProvider::for_value(&value))
    });

    app_icon.add_controller(drag_source.clone());

    // Drop target and hover state.

    let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::COPY);

    let folder_for_enter = folder_card.clone();

    drop_target.connect_enter(move |_, _, _| {
        folder_for_enter.add_css_class("appdrop-drop-hover");
        gtk::gdk::DragAction::COPY
    });

    let folder_for_leave = folder_card.clone();

    drop_target.connect_leave(move |_| {
        folder_for_leave.remove_css_class("appdrop-drop-hover");
    });

    let selected_for_drop = selected_path.clone();
    let status_for_drop = status.clone();
    let uninstall_for_drop = uninstall_button.clone();
    let folder_icon_for_drop = folder_icon.clone();
    let folder_hint_for_drop = folder_hint.clone();
    let drag_for_drop = drag_source.clone();
    let arrow_for_drop = arrow.clone();
    let folder_card_for_drop = folder_card.clone();
    let choose_for_drop = choose_button.clone();
    let app_hint_for_drop = app_hint.clone();
    let status_icon_for_drop = status_icon.clone();

    drop_target.connect_drop(move |_, value, _, _| {
        if value.get::<String>().ok().as_deref() != Some("appdrop-install") {
            return false;
        }

        let Some(path) = selected_for_drop.borrow().clone() else {
            status_for_drop.set_text("Choose or open an AppImage before installing.");
            return false;
        };

        status_for_drop.set_text("Installing application…");
        status_icon_for_drop.set_icon_name(Some("content-loading-symbolic"));

        match metadata::inspect(&path) {
            Ok(info) => {
                if installer::is_installed(&info) {
                    status_for_drop.set_text(&format!("{} is already installed.", info.name));

                    set_installed_view(
                        true,
                        &arrow_for_drop,
                        &folder_card_for_drop,
                        &choose_for_drop,
                        &uninstall_for_drop,
                        &drag_for_drop,
                        &app_hint_for_drop,
                    );

                    status_icon_for_drop.set_icon_name(Some("emblem-ok-symbolic"));

                    return true;
                }

                match installer::install(&path, &info) {
                    Ok(installed_path) => {
                        status_for_drop.set_text(&format!(
                            "{} installed successfully · {}",
                            info.name,
                            installed_path.display(),
                        ));

                        set_installed_view(
                            true,
                            &arrow_for_drop,
                            &folder_card_for_drop,
                            &choose_for_drop,
                            &uninstall_for_drop,
                            &drag_for_drop,
                            &app_hint_for_drop,
                        );

                        folder_icon_for_drop.set_icon_name(Some("emblem-ok-symbolic"));
                        folder_hint_for_drop.set_text("Installed!");

                        status_icon_for_drop.set_icon_name(Some("emblem-ok-symbolic"));
                    }
                    Err(error) => {
                        status_for_drop.set_text(&format!("Installation failed: {error}"));
                        status_icon_for_drop.set_icon_name(Some("dialog-error-symbolic"));
                    }
                }
            }
            Err(error) => {
                status_for_drop.set_text(&format!("Could not inspect AppImage: {error}"));
                status_icon_for_drop.set_icon_name(Some("dialog-error-symbolic"));
            }
        }

        true
    });

    folder_card.add_controller(drop_target);

    // File chooser.
    let window_for_chooser = window.clone();
    let icon_for_chooser = app_icon.clone();
    let name_for_chooser = app_name.clone();
    let details_for_chooser = app_details.clone();
    let status_for_chooser = status.clone();
    let button_for_chooser = uninstall_button.clone();
    let selected_for_chooser = selected_path.clone();
    let arrow_for_chooser = arrow.clone();
    let folder_for_chooser = folder_card.clone();
    let choose_for_chooser = choose_button.clone();
    let drag_for_chooser = drag_source.clone();
    let hint_for_chooser = app_hint.clone();
    let folder_icon_for_chooser = folder_icon.clone();
    let folder_hint_for_chooser = folder_hint.clone();
    let status_icon_for_chooser = status_icon.clone();

    choose_button.connect_clicked(move |_| {
        let dialog = gtk::FileChooserNative::builder()
            .title("Choose an AppImage")
            .transient_for(&window_for_chooser)
            .action(gtk::FileChooserAction::Open)
            .accept_label("Select")
            .cancel_label("Cancel")
            .build();

        let filter = gtk::FileFilter::new();
        filter.set_name(Some("AppImage files"));
        filter.add_pattern("*.AppImage");
        filter.add_pattern("*.appimage");
        dialog.add_filter(&filter);

        let icon = icon_for_chooser.clone();
        let name = name_for_chooser.clone();
        let details = details_for_chooser.clone();
        let status = status_for_chooser.clone();
        let button = button_for_chooser.clone();
        let selected = selected_for_chooser.clone();
        let arrow = arrow_for_chooser.clone();
        let folder = folder_for_chooser.clone();
        let choose = choose_for_chooser.clone();
        let drag = drag_for_chooser.clone();
        let hint = hint_for_chooser.clone();
        let folder_icon = folder_icon_for_chooser.clone();
        let folder_hint = folder_hint_for_chooser.clone();
        let status_icon = status_icon_for_chooser.clone();

        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                match dialog.file().and_then(|file| file.path()) {
                    Some(path) => {
                        folder_icon.set_icon_name(Some("folder-symbolic"));
                        folder_hint.set_text("Drop here to install");
                        status_icon.set_icon_name(Some("emblem-information-symbolic"));

                        inspect_into_ui(
                            path, &icon, &name, &details, &status, &arrow, &folder, &choose,
                            &button, &drag, &hint, &selected,
                        );
                    }
                    None => {
                        status.set_text("Please select a local file.");
                    }
                }
            }

            dialog.destroy();
        });

        dialog.show();
    });

    // Uninstaller.
    let selected_for_uninstall = selected_path.clone();
    let status_for_uninstall = status.clone();
    let button_for_uninstall = uninstall_button.clone();
    let icon_for_uninstall = app_icon.clone();
    let name_for_uninstall = app_name.clone();
    let details_for_uninstall = app_details.clone();
    let arrow_for_uninstall = arrow.clone();
    let folder_for_uninstall = folder_card.clone();
    let choose_for_uninstall = choose_button.clone();
    let drag_for_uninstall = drag_source.clone();
    let hint_for_uninstall = app_hint.clone();
    let folder_icon_for_uninstall = folder_icon.clone();
    let folder_hint_for_uninstall = folder_hint.clone();
    let status_icon_for_uninstall = status_icon.clone();

    uninstall_button.connect_clicked(move |_| {
        let Some(path) = selected_for_uninstall.borrow().clone() else {
            status_for_uninstall.set_text("Choose an AppImage first.");
            return;
        };

        let info = match metadata::inspect(&path) {
            Ok(info) => info,
            Err(error) => {
                status_for_uninstall.set_text(&format!("Could not inspect application: {error}"));
                return;
            }
        };

        match installer::uninstall(&info) {
            Ok(()) => {
                status_for_uninstall.set_text(&format!("{} has been uninstalled.", info.name));

                status_icon_for_uninstall.set_icon_name(Some("emblem-ok-symbolic"));

                set_installed_view(
                    false,
                    &arrow_for_uninstall,
                    &folder_for_uninstall,
                    &choose_for_uninstall,
                    &button_for_uninstall,
                    &drag_for_uninstall,
                    &hint_for_uninstall,
                );

                icon_for_uninstall.set_icon_name(Some("application-x-executable-symbolic"));
                name_for_uninstall.set_text("Your AppImage");
                details_for_uninstall
                    .set_text("Application removed. The original AppImage is untouched.");

                folder_icon_for_uninstall.set_icon_name(Some("folder-symbolic"));
                folder_hint_for_uninstall.set_text("Drop here to install");

                *selected_for_uninstall.borrow_mut() = None;
            }
            Err(error) => {
                status_for_uninstall.set_text(&format!("Uninstallation failed: {error}"));

                status_icon_for_uninstall.set_icon_name(Some("dialog-error-symbolic"));
            }
        }
    });

    window.present();

    // Open directly with an AppImage argument.
    if let Some(path) = initial_path {
        inspect_into_ui(
            path,
            &app_icon,
            &app_name,
            &app_details,
            &status,
            &arrow,
            &folder_card,
            &choose_button,
            &uninstall_button,
            &drag_source,
            &app_hint,
            &selected_path,
        );
    }
}
