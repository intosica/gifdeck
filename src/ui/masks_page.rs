//! "Masks" page: saved GIF / PNG / WebP shapes that widgets can be clipped to (animated ones play).

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::anim_view::AnimView;
use crate::ctx::AppCtx;
use crate::media;

pub struct MasksPage {
    ctx: Rc<AppCtx>,
    pub root: gtk::ScrolledWindow,
    list: gtk::ListBox,
}

impl MasksPage {
    pub fn new(ctx: &Rc<AppCtx>) -> Rc<MasksPage> {
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
        let hint = gtk::Label::builder()
            .label("No masks yet.\nPress Add or drop GIF, PNG or WebP files here.\nThe alpha channel is the shape; animated GIFs and WebPs animate it.")
            .justify(gtk::Justification::Center)
            .css_classes(["dim-label"])
            .margin_top(24)
            .margin_bottom(24)
            .build();
        list.set_placeholder(Some(&hint));
        let add = gtk::Button::builder()
            .child(&adw::ButtonContent::builder().icon_name("list-add-symbolic").label("Add").build())
            .css_classes(["suggested-action"])
            .build();
        let group = adw::PreferencesGroup::builder()
            .title("Alpha masks")
            .description("Pick one in a widget's settings to clip it to the shape.")
            .header_suffix(&add)
            .build();
        group.add(&list);
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).margin_top(12).margin_bottom(18).margin_start(12).margin_end(12).build();
        content.append(&group);
        let clamp = adw::Clamp::builder().maximum_size(640).child(&content).build();
        let root = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&clamp).build();

        {
            let ctx = ctx.clone();
            add.connect_clicked(move |btn| {
                let filter = gtk::FileFilter::new();
                filter.set_name(Some("GIF, PNG and WebP images"));
                filter.add_mime_type("image/png");
                filter.add_mime_type("image/webp");
                filter.add_mime_type("image/gif");
                let filters = gio::ListStore::new::<gtk::FileFilter>();
                filters.append(&filter);
                let dialog = gtk::FileDialog::builder().title("Add alpha masks").modal(true).filters(&filters).default_filter(&filter).build();
                if let Some(d) = ctx.state.borrow().storage.dir("Masks") {
                    dialog.set_initial_folder(Some(&gio::File::for_path(d)));
                }
                let parent = btn.root().and_downcast::<gtk::Window>();
                let ctx = ctx.clone();
                dialog.open_multiple(parent.as_ref(), gio::Cancellable::NONE, move |res| {
                    if let Ok(files) = res {
                        ctx.import_masks(files.iter::<gio::File>().flatten().collect());
                    }
                });
            });
        }
        let page = Rc::new(MasksPage { ctx: ctx.clone(), root, list });
        page.rebuild();
        page
    }

    pub fn rebuild(&self) {
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        self.ctx.sync_masks();
        let masks = self.ctx.state.borrow().masks.clone();
        for m in masks {
            let row = adw::ActionRow::builder().title(glib::markup_escape_text(&m.name)).subtitle(glib::markup_escape_text(&m.path.to_string_lossy())).build();
            let preview = gtk::Box::builder().width_request(56).height_request(56).valign(gtk::Align::Center).build();
            let path = m.path.clone();
            let slot = preview.clone();
            glib::spawn_future_local(async move {
                match media::load(path).await {
                    Ok(anim) => {
                        let k = (56.0 / anim.width.max(anim.height) as f64).min(1.0);
                        let view = AnimView::new(anim.clone(), ((anim.width as f64 * k).round() as i32).max(1), ((anim.height as f64 * k).round() as i32).max(1), None);
                        view.set_halign(gtk::Align::Center);
                        view.set_valign(gtk::Align::Center);
                        view.set_hexpand(true);
                        view.play();
                        slot.append(&view);
                    }
                    Err(_) => slot.append(&gtk::Image::from_icon_name("image-missing-symbolic")),
                }
            });
            row.add_prefix(&preview);
            let del = gtk::Button::builder().icon_name("user-trash-symbolic").tooltip_text("Remove mask").valign(gtk::Align::Center).css_classes(["flat"]).build();
            let (ctx, id) = (self.ctx.clone(), m.id.clone());
            del.connect_clicked(move |_| ctx.remove_mask(&id));
            row.add_suffix(&del);
            super::rename_on_right_click(&row, &self.ctx, m.path.clone(), m.name.clone());
            self.list.append(&row);
        }
    }
}

/// Hovering a mask in `row`'s dropdown shows a small live preview of it beside the list.
/// `paths[i]` belongs to list entry `i + 1` (entry 0 is "None").
pub fn hover_preview(row: &adw::ComboRow, paths: Vec<std::path::PathBuf>) {
    use std::cell::{Cell, RefCell};

    let factory = gtk::SignalListItemFactory::new();
    let paths = Rc::new(paths);
    let row_weak = row.downgrade();
    factory.connect_setup(move |_, obj| {
        let Some(item) = obj.downcast_ref::<gtk::ListItem>() else { return };
        let label = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build();
        let check = gtk::Image::from_icon_name("object-select-symbolic");
        let line = gtk::Box::builder().spacing(8).build();
        line.append(&label);
        line.append(&check);
        item.set_child(Some(&line));

        let token = Rc::new(Cell::new(0u32));
        let popover: Rc<RefCell<Option<gtk::Popover>>> = Rc::new(RefCell::new(None));
        let hide = {
            let (token, popover) = (token.clone(), popover.clone());
            move || {
                token.set(token.get() + 1);
                if let Some(p) = popover.borrow_mut().take() {
                    p.popdown();
                    p.unparent();
                }
            }
        };

        let motion = gtk::EventControllerMotion::new();
        {
            let (item, paths, token, popover, label, hide) = (item.downgrade(), paths.clone(), token.clone(), popover.clone(), label.downgrade(), hide.clone());
            motion.connect_enter(move |_, _, _| {
                hide();
                let (Some(item), Some(label)) = (item.upgrade(), label.upgrade()) else { return };
                let Some(path) = (item.position() as usize).checked_sub(1).and_then(|i| paths.get(i)).cloned() else { return };
                let (mine, token, popover) = (token.get(), token.clone(), popover.clone());
                glib::spawn_future_local(async move {
                    let Ok(anim) = media::load(path).await else { return };
                    if token.get() != mine || !label.is_mapped() {
                        return; // the pointer moved on
                    }
                    let k = (160.0 / anim.width.max(anim.height) as f64).min(1.0);
                    let view = AnimView::new(anim.clone(), ((anim.width as f64 * k).round() as i32).max(1), ((anim.height as f64 * k).round() as i32).max(1), None);
                    view.play();
                    let frame = gtk::Box::builder().margin_top(6).margin_bottom(6).margin_start(6).margin_end(6).css_classes(["gifdeck-preview"]).build();
                    frame.append(&view);
                    let pop = gtk::Popover::builder().child(&frame).position(gtk::PositionType::Right).autohide(false).can_target(false).build();
                    pop.set_parent(&label);
                    pop.popup();
                    *popover.borrow_mut() = Some(pop);
                });
            });
        }
        {
            let hide = hide.clone();
            motion.connect_leave(move |_| hide());
        }
        line.add_controller(motion);
        line.connect_unmap(move |_| hide());
    });
    factory.connect_bind(move |_, obj| {
        let Some(item) = obj.downcast_ref::<gtk::ListItem>() else { return };
        let Some(line) = item.child().and_downcast::<gtk::Box>() else { return };
        if let (Some(label), Some(check), Some(name)) = (line.first_child().and_downcast::<gtk::Label>(), line.last_child(), item.item().and_downcast::<gtk::StringObject>()) {
            label.set_text(&name.string());
            check.set_visible(row_weak.upgrade().is_some_and(|r| r.selected() == item.position()));
        }
    });
    row.set_list_factory(Some(&factory));
}
