//! "Select Application" dialog (like Caffeine's) and small helpers to show an [`AppRef`].

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;

use crate::model::AppRef;

/// Icon for an app (falls back to a generic one).
pub fn icon_of(app: &AppRef) -> gtk::Image {
    match gio::Icon::for_string(&app.icon) {
        Ok(icon) if !app.icon.is_empty() => gtk::Image::from_gicon(&icon),
        _ => gtk::Image::from_icon_name("application-x-executable-symbolic"),
    }
}

fn app_ref(info: &gio::AppInfo) -> AppRef {
    let id = info.id().map(|s| s.to_string()).unwrap_or_default();
    let mut keys = vec![id.trim_end_matches(".desktop").to_lowercase(), info.name().to_lowercase()];
    if let Some(d) = info.downcast_ref::<gio::DesktopAppInfo>() {
        keys.extend(d.startup_wm_class().map(|c| c.to_lowercase()));
    }
    let exe = info.executable();
    keys.extend(exe.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    keys.retain(|k| !k.is_empty());
    keys.dedup();
    AppRef {
        id,
        name: info.name().to_string(),
        icon: info.icon().and_then(|i| gio::prelude::IconExt::to_string(&i)).map(|s| s.to_string()).unwrap_or_default(),
        keys,
    }
}

/// Modal list of installed applications; calls `on_pick` with the chosen one.
pub fn pick_app(parent: &impl IsA<gtk::Widget>, on_pick: impl Fn(AppRef) + 'static) {
    let mut apps: Vec<AppRef> = gio::AppInfo::all().iter().filter(|i| i.should_show()).map(app_ref).collect();
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps.dedup_by(|a, b| a.id == b.id);
    let apps = Rc::new(apps);

    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).build();
    for a in apps.iter() {
        let row = adw::ActionRow::builder().title(gtk::glib::markup_escape_text(&a.name)).build();
        row.add_prefix(&icon_of(a));
        list.append(&row);
    }
    let query = Rc::new(RefCell::new(String::new()));
    {
        let (apps, query) = (apps.clone(), query.clone());
        list.set_filter_func(move |row| {
            let q = query.borrow();
            q.is_empty() || apps.get(row.index() as usize).is_some_and(|a| a.name.to_lowercase().contains(q.as_str()))
        });
    }
    let search = gtk::SearchEntry::builder().placeholder_text("Search applications").margin_start(12).margin_end(12).margin_top(6).margin_bottom(6).build();
    {
        let (list, query) = (list.clone(), query.clone());
        search.connect_search_changed(move |s| {
            *query.borrow_mut() = s.text().to_lowercase();
            list.invalidate_filter();
        });
    }

    let cancel = gtk::Button::with_label("Cancel");
    let select = gtk::Button::builder().label("Select").css_classes(["suggested-action"]).sensitive(false).build();
    let header = adw::HeaderBar::builder().show_end_title_buttons(false).show_start_title_buttons(false).title_widget(&adw::WindowTitle::new("Select Application", "")).build();
    header.pack_start(&cancel);
    header.pack_end(&select);
    let scroller = gtk::ScrolledWindow::builder().vexpand(true).child(&list).build();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(&search);
    body.append(&scroller);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&body));
    let dialog = adw::Dialog::builder().title("Select Application").content_width(380).content_height(520).child(&toolbar).build();

    {
        let select = select.clone();
        list.connect_row_selected(move |_, row| select.set_sensitive(row.is_some()));
    }
    let choose: Rc<dyn Fn()> = {
        let (list, apps, dialog) = (list.clone(), apps.clone(), dialog.clone());
        Rc::new(move || {
            if let Some(a) = list.selected_row().and_then(|r| apps.get(r.index() as usize).cloned()) {
                dialog.close();
                on_pick(a);
            }
        })
    };
    {
        let choose = choose.clone();
        select.connect_clicked(move |_| choose());
    }
    list.connect_row_activated(move |_, _| choose());
    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| {
            dialog.close();
        });
    }
    dialog.present(Some(parent));
}
