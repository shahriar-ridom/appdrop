mod appimage;
mod installer;
mod library;
mod metadata;
mod version;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use metadata::{AppMetadata, MetadataError};

type SelectedPath = Rc<RefCell<Option<PathBuf>>>;

/// The one AppDrop window, kept so a second launch can reuse it.
#[derive(Clone)]
struct AppWindow {
    window: adw::ApplicationWindow,
    stack: adw::ViewStack,
    ui: Ui,
}

impl AppWindow {
    fn show_library(&self) {
        self.stack.set_visible_child_name("library");
        self.window.present();
    }

    fn show_installer(&self, path: Option<PathBuf>) {
        self.stack.set_visible_child_name("install");
        self.window.present();

        if let Some(path) = path {
            self.ui.inspect(path);
        }
    }
}

type CurrentWindow = Rc<RefCell<Option<AppWindow>>>;

/// The existing window if AppDrop is already running, otherwise a new one.
fn window_for(app: &adw::Application, current: &CurrentWindow) -> AppWindow {
    if !app.windows().is_empty()
        && let Some(existing) = current.borrow().clone()
    {
        return existing;
    }

    let created = build_ui(app);
    *current.borrow_mut() = Some(created.clone());
    created
}

fn main() {
    let app = adw::Application::builder()
        .application_id("dev.appdrop.Installer")
        .flags(gtk::gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    let current: CurrentWindow = Rc::new(RefCell::new(None));

    // Launching AppDrop itself opens the library of installed apps.
    let current_for_activate = current.clone();
    app.connect_activate(move |app| {
        window_for(app, &current_for_activate).show_library();
    });

    // Opening an AppImage (double click, "Open with") goes straight to the
    // installer.
    app.connect_open(move |app, files, _hint| {
        let path = files.first().and_then(|file| file.path());
        window_for(app, &current).show_installer(path);
    });

    app.run();
}

/// Handles to every widget the install flow updates, plus shared state.
#[derive(Clone)]
struct Ui {
    app_icon: gtk::Image,
    app_name: gtk::Label,
    app_details: gtk::Label,
    app_hint: gtk::Label,
    status: gtk::Label,
    status_icon: gtk::Image,
    arrow: gtk::Image,
    folder_card: gtk::Box,
    folder_icon: gtk::Image,
    folder_hint: gtk::Label,
    choose_button: gtk::Button,
    update_button: gtk::Button,
    uninstall_button: gtk::Button,
    drag_source: gtk::DragSource,
    selected_path: SelectedPath,
    busy: Rc<Cell<bool>>,
}

enum InstallOutcome {
    AlreadyInstalled(String),
    Installed(String, PathBuf),
}

impl Ui {
    fn set_installed_view(&self, installed: bool) {
        self.arrow.set_visible(!installed);
        self.folder_card.set_visible(!installed);

        self.uninstall_button.set_visible(installed);
        self.uninstall_button.set_sensitive(installed);

        if !installed {
            self.update_button.set_visible(false);
        }

        self.drag_source.set_actions(if installed {
            gtk::gdk::DragAction::empty()
        } else {
            gtk::gdk::DragAction::COPY
        });

        self.app_hint.set_text(if installed {
            "Installed application"
        } else {
            "Drag this icon to install"
        });
    }

    fn reset_folder(&self) {
        self.folder_icon.set_icon_name(Some("folder-symbolic"));
        self.folder_hint.set_text("Drop here to install");
    }

    fn set_status(&self, icon: &str, text: &str) {
        self.status_icon.set_icon_name(Some(icon));
        self.status.set_text(text);
    }

    /// Marks a background job as running. Returns false if one already is.
    fn begin(&self, icon: &str, text: &str) -> bool {
        if self.busy.replace(true) {
            return false;
        }

        self.choose_button.set_sensitive(false);
        self.update_button.set_sensitive(false);
        self.uninstall_button.set_sensitive(false);
        self.set_status(icon, text);
        true
    }

    fn end(&self) {
        self.busy.set(false);
        self.choose_button.set_sensitive(true);
        self.update_button.set_sensitive(true);
        self.uninstall_button.set_sensitive(true);
    }

    /// Inspect `path` on a worker thread, then show the result.
    fn inspect(&self, path: PathBuf) {
        if !self.begin("content-loading-symbolic", "Inspecting application…") {
            return;
        }

        self.reset_folder();

        let ui = self.clone();

        gtk::glib::spawn_future_local(async move {
            let worker_path = path.clone();
            let result = gtk::gio::spawn_blocking(move || metadata::inspect(&worker_path)).await;

            ui.end();

            match result {
                Ok(result) => ui.show_inspection(path, result),
                Err(_) => ui.set_status("dialog-error-symbolic", "Inspection failed unexpectedly."),
            }
        });
    }

    fn show_inspection(&self, path: PathBuf, result: Result<AppMetadata, MetadataError>) {
        match result {
            Ok(info) => {
                let icon_name = info
                    .icon
                    .as_deref()
                    .filter(|icon| !icon.is_empty() && !icon.contains('/'))
                    .unwrap_or("application-x-executable-symbolic");

                self.app_icon.set_icon_name(Some(icon_name));
                self.app_name.set_text(&info.name);

                let categories = if info.categories.is_empty() {
                    "Not specified".to_string()
                } else {
                    info.categories.join(", ")
                };

                self.app_details.set_text(&format!(
                    "Version: {}\n\
                     Desktop entry: {}\n\
                     Icon: {}\n\
                     Categories: {}\n\
                     Launch command: {}\n\
                     SquashFS offset: {} bytes",
                    info.version.as_deref().unwrap_or("Unknown"),
                    info.desktop_file,
                    info.icon.as_deref().unwrap_or("Not specified"),
                    categories,
                    info.exec.as_deref().unwrap_or("Not specified"),
                    info.squashfs_offset,
                ));

                let installed_version = installer::installed_version(&info);
                let installed = installed_version.is_some();
                self.set_installed_view(installed);

                let installed_version = installed_version.flatten();
                let offer = installed.then(|| {
                    version::update_label(installed_version.as_deref(), info.version.as_deref())
                });

                match offer.flatten() {
                    Some(label) => {
                        self.update_button.set_label(&label);
                        self.update_button.set_visible(true);
                    }
                    None => self.update_button.set_visible(false),
                }

                let status = match (installed, self.update_button.is_visible()) {
                    (false, _) => {
                        "Ready to install. Drag the app icon into Applications.".to_string()
                    }
                    (true, true) => format!(
                        "{} is already installed{}. You can replace it with this file.",
                        info.name,
                        installed_version
                            .as_deref()
                            .map(|v| format!(" (version {v})"))
                            .unwrap_or_default(),
                    ),
                    (true, false) => "This application is managed by AppDrop.".to_string(),
                };

                self.set_status("emblem-information-symbolic", &status);

                *self.selected_path.borrow_mut() = Some(path);
            }
            Err(error) => {
                self.app_icon
                    .set_icon_name(Some("application-x-executable-symbolic"));
                self.app_name.set_text("Unsupported application");
                self.app_details
                    .set_text("AppDrop couldn't read this application's metadata.");

                self.set_status(
                    "dialog-error-symbolic",
                    &format!("Inspection failed: {error}"),
                );

                self.set_installed_view(false);
                *self.selected_path.borrow_mut() = None;
            }
        }
    }

    /// Install the selected AppImage on a worker thread.
    fn install(&self) {
        let Some(path) = self.selected_path.borrow().clone() else {
            self.set_status(
                "dialog-error-symbolic",
                "Choose or open an AppImage before installing.",
            );
            return;
        };

        if !self.begin("content-loading-symbolic", "Installing application…") {
            return;
        }

        let ui = self.clone();

        gtk::glib::spawn_future_local(async move {
            let result = gtk::gio::spawn_blocking(move || -> Result<InstallOutcome, String> {
                let info = metadata::inspect(&path)
                    .map_err(|e| format!("Could not inspect AppImage: {e}"))?;

                if installer::is_installed(&info) {
                    return Ok(InstallOutcome::AlreadyInstalled(info.name));
                }

                let installed = installer::install(&path, &info)
                    .map_err(|e| format!("Installation failed: {e}"))?;

                Ok(InstallOutcome::Installed(info.name, installed))
            })
            .await;

            ui.end();

            match result {
                Ok(Ok(InstallOutcome::AlreadyInstalled(name))) => {
                    ui.set_installed_view(true);
                    ui.set_status(
                        "emblem-ok-symbolic",
                        &format!("{name} is already installed."),
                    );
                }
                Ok(Ok(InstallOutcome::Installed(name, installed))) => {
                    ui.set_installed_view(true);
                    ui.folder_icon.set_icon_name(Some("emblem-ok-symbolic"));
                    ui.folder_hint.set_text("Installed!");
                    ui.set_status(
                        "emblem-ok-symbolic",
                        &format!("{name} installed successfully · {}", installed.display()),
                    );
                }
                Ok(Err(message)) => ui.set_status("dialog-error-symbolic", &message),
                Err(_) => {
                    ui.set_status("dialog-error-symbolic", "Installation failed unexpectedly.")
                }
            }
        });
    }

    /// Replace the installed copy with the selected AppImage.
    fn update(&self) {
        let Some(path) = self.selected_path.borrow().clone() else {
            self.set_status("dialog-error-symbolic", "Choose an AppImage first.");
            return;
        };

        if !self.begin("content-loading-symbolic", "Updating application…") {
            return;
        }

        let ui = self.clone();

        gtk::glib::spawn_future_local(async move {
            let worker_path = path.clone();

            let result = gtk::gio::spawn_blocking(move || -> Result<AppMetadata, String> {
                let info = metadata::inspect(&worker_path)
                    .map_err(|e| format!("Could not inspect AppImage: {e}"))?;

                installer::update(&worker_path, &info)
                    .map_err(|e| format!("Update failed: {e}"))?;

                Ok(info)
            })
            .await;

            ui.end();

            match result {
                Ok(Ok(info)) => {
                    let message = match &info.version {
                        Some(version) => format!("{} is now version {version}.", info.name),
                        None => format!("{} has been updated.", info.name),
                    };

                    ui.show_inspection(path, Ok(info));
                    ui.set_status("emblem-ok-symbolic", &message);
                }
                Ok(Err(message)) => ui.set_status("dialog-error-symbolic", &message),
                Err(_) => ui.set_status("dialog-error-symbolic", "Update failed unexpectedly."),
            }
        });
    }

    /// Uninstall the selected application on a worker thread.
    fn uninstall(&self) {
        let Some(path) = self.selected_path.borrow().clone() else {
            self.set_status("dialog-error-symbolic", "Choose an AppImage first.");
            return;
        };

        if !self.begin("content-loading-symbolic", "Uninstalling application…") {
            return;
        }

        let ui = self.clone();

        gtk::glib::spawn_future_local(async move {
            let result = gtk::gio::spawn_blocking(move || -> Result<String, String> {
                let info = metadata::inspect(&path)
                    .map_err(|e| format!("Could not inspect application: {e}"))?;

                installer::uninstall(&info).map_err(|e| format!("Uninstallation failed: {e}"))?;

                Ok(info.name)
            })
            .await;

            ui.end();

            match result {
                Ok(Ok(name)) => {
                    ui.set_installed_view(false);
                    ui.app_icon
                        .set_icon_name(Some("application-x-executable-symbolic"));
                    ui.app_name.set_text("Your AppImage");
                    ui.app_details
                        .set_text("Application removed. The original AppImage is untouched.");
                    ui.reset_folder();
                    ui.set_status(
                        "emblem-ok-symbolic",
                        &format!("{name} has been uninstalled."),
                    );

                    *ui.selected_path.borrow_mut() = None;
                }
                Ok(Err(message)) => ui.set_status("dialog-error-symbolic", &message),
                Err(_) => ui.set_status(
                    "dialog-error-symbolic",
                    "Uninstallation failed unexpectedly.",
                ),
            }
        });
    }
}

fn build_ui(app: &adw::Application) -> AppWindow {
    // Follow the system light/dark preference.
    // Use `adw::ColorScheme::PreferDark` here if you want to always default to dark.
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::Default);

    // AppDrop's theme (uses libadwaita named colors so it adapts to any distro/theme)

    let css_provider = gtk::CssProvider::new();

    css_provider.load_from_data(
        r#"
        .appdrop-heading {
            font-size: 28px;
            font-weight: 800;
            letter-spacing: -0.5px;
        }

        .appdrop-subtitle {
            opacity: 0.65;
            font-size: 14px;
        }

        .appdrop-eyebrow {
            color: #a78bfa;
            font-size: 10px;
            font-weight: 800;
            letter-spacing: 1.5px;
        }

        .appdrop-card {
            background-color: alpha(@window_fg_color, 0.05);
            border: 1px solid alpha(@window_fg_color, 0.10);
            border-radius: 18px;
            box-shadow: 0 2px 8px alpha(black, 0.18);
        }

        .appdrop-icon-tile {
            background: alpha(#8b5cf6, 0.18);
            border: 1px solid alpha(#a78bfa, 0.30);
            border-radius: 20px;
            padding: 18px;
        }

        .appdrop-app-name {
            font-size: 19px;
            font-weight: 700;
        }

        .appdrop-muted {
            opacity: 0.65;
        }

        .appdrop-section-title {
            color: #a78bfa;
            font-size: 11px;
            font-weight: 700;
            letter-spacing: 0.8px;
        }

        .appdrop-drop-hover {
            background: alpha(#8b5cf6, 0.20);
            border: 2px solid #a78bfa;
        }

        button.appdrop-primary {
            background: #8b5cf6;
            color: white;
            min-height: 44px;
            font-weight: 700;
            border-radius: 12px;
            border: none;
        }

        button.appdrop-primary:hover {
            background: #a78bfa;
        }

        button.appdrop-secondary {
            background: alpha(@window_fg_color, 0.08);
            min-height: 42px;
            border-radius: 12px;
            border: none;
        }

        button.appdrop-secondary:hover {
            background: alpha(@window_fg_color, 0.14);
        }

        .appdrop-status {
            opacity: 0.65;
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
        .default_width(640)
        .default_height(700)
        .build();

    let selected_path: SelectedPath = Rc::new(RefCell::new(None));

    let content = gtk::Box::new(gtk::Orientation::Vertical, 22);
    content.set_margin_top(8);
    content.set_margin_bottom(26);
    content.set_margin_start(24);
    content.set_margin_end(24);

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
    app_card_content.set_hexpand(true);
    app_card_content.set_vexpand(true);
    app_card_content.set_margin_top(28);
    app_card_content.set_margin_bottom(28);
    app_card_content.set_margin_start(20);
    app_card_content.set_margin_end(20);

    app_card_content.append(&icon_tile);
    app_card_content.append(&app_name);
    app_card_content.append(&app_hint);

    let app_card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    app_card.add_css_class("appdrop-card");
    app_card.set_size_request(220, 230);
    app_card.append(&app_card_content);

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
    folder_content.set_hexpand(true);
    folder_content.set_vexpand(true);
    folder_content.set_margin_top(28);
    folder_content.set_margin_bottom(28);
    folder_content.set_margin_start(20);
    folder_content.set_margin_end(20);

    folder_content.append(&folder_icon);
    folder_content.append(&folder_name);
    folder_content.append(&folder_hint);

    let folder_card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    folder_card.add_css_class("appdrop-card");
    folder_card.set_size_request(220, 230);
    folder_card.append(&folder_content);

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
    details_content.set_hexpand(true);
    details_content.set_margin_top(18);
    details_content.set_margin_bottom(18);
    details_content.set_margin_start(20);
    details_content.set_margin_end(20);
    details_content.append(&details_heading);
    details_content.append(&app_details);

    let details_card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    details_card.add_css_class("appdrop-card");
    details_card.append(&details_content);

    // Actions.

    let choose_button = gtk::Button::with_label("Choose AppImage");
    choose_button.add_css_class("appdrop-primary");
    choose_button.set_halign(gtk::Align::Fill);

    let update_button = gtk::Button::with_label("Update");
    update_button.add_css_class("appdrop-primary");
    update_button.set_halign(gtk::Align::Fill);
    update_button.set_visible(false);

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
    content.append(&update_button);
    content.append(&choose_button);
    content.append(&uninstall_button);
    content.append(&status_row);

    // Window chrome: header bar + centered, scrollable content.
    // Works the same on GNOME, Hyprland, KDE, etc.

    let header_bar = adw::HeaderBar::new();
    header_bar.add_css_class("flat");

    let clamp = adw::Clamp::builder()
        .maximum_size(600)
        .child(&content)
        .build();

    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&clamp)
        .build();

    // Two pages: the library of installed apps (the default when AppDrop is
    // launched) and the installer (what opening an AppImage shows).
    let ui_slot: Rc<RefCell<Option<Ui>>> = Rc::new(RefCell::new(None));
    let toasts = adw::ToastOverlay::new();
    let stack = adw::ViewStack::new();

    let on_install: Rc<dyn Fn()> = {
        let stack = stack.clone();
        let ui_slot = ui_slot.clone();

        Rc::new(move || {
            stack.set_visible_child_name("install");

            if let Some(ui) = ui_slot.borrow().as_ref() {
                ui.choose_button.emit_clicked();
            }
        })
    };

    let on_update: Rc<dyn Fn(PathBuf)> = {
        let stack = stack.clone();
        let ui_slot = ui_slot.clone();

        Rc::new(move |path| {
            stack.set_visible_child_name("install");

            if let Some(ui) = ui_slot.borrow().as_ref() {
                ui.inspect(path);
            }
        })
    };

    let library = library::Library::new(&window, &toasts, on_install, on_update);
    library.refresh();

    stack.add_titled_with_icon(
        &library.widget,
        Some("library"),
        "Library",
        "view-list-symbolic",
    );
    stack.add_titled_with_icon(&scrolled, Some("install"), "Install", "list-add-symbolic");

    let library_for_switch = library.clone();
    stack.connect_visible_child_name_notify(move |stack| {
        if stack.visible_child_name().as_deref() == Some("library") {
            library_for_switch.refresh();
        }
    });

    let switcher = adw::ViewSwitcher::builder()
        .stack(&stack)
        .policy(adw::ViewSwitcherPolicy::Wide)
        .build();
    header_bar.set_title_widget(Some(&switcher));

    toasts.set_child(Some(&stack));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header_bar);
    toolbar.set_content(Some(&toasts));

    window.set_content(Some(&toolbar));

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

    let ui = Ui {
        app_icon: app_icon.clone(),
        app_name,
        app_details,
        app_hint,
        status,
        status_icon,
        arrow,
        folder_card: folder_card.clone(),
        folder_icon,
        folder_hint,
        choose_button: choose_button.clone(),
        update_button: update_button.clone(),
        uninstall_button: uninstall_button.clone(),
        drag_source: drag_source.clone(),
        selected_path,
        busy: Rc::new(Cell::new(false)),
    };

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

    let ui_for_drop = ui.clone();

    drop_target.connect_drop(move |_, value, _, _| {
        if value.get::<String>().ok().as_deref() != Some("appdrop-install") {
            return false;
        }

        ui_for_drop.install();
        true
    });

    folder_card.add_controller(drop_target);

    // File chooser.
    let window_for_chooser = window.clone();
    let ui_for_chooser = ui.clone();

    choose_button.connect_clicked(move |_| {
        let ui = ui_for_chooser.clone();
        library::choose_appimage(&window_for_chooser, "Choose an AppImage", move |path| {
            ui.inspect(path)
        });
    });

    // Update and uninstall.
    let ui_for_update = ui.clone();
    update_button.connect_clicked(move |_| ui_for_update.update());

    let ui_for_uninstall = ui.clone();
    uninstall_button.connect_clicked(move |_| ui_for_uninstall.uninstall());

    *ui_slot.borrow_mut() = Some(ui.clone());

    AppWindow { window, stack, ui }
}
