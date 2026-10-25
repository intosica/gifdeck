//! "Masks" page: saved GIF / PNG shapes that widgets can be clipped to (animated ones play).

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
            .label("No masks yet.\nPress Add or drop GIF or PNG files here.\nThe alpha channel is the shape; animated GIFs animate it.")
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
                filter.set_name(Some("GIF and PNG images"));
                filter.add_mime_type("image/png");
                filter.add_mime_type("image/gif");
                let filters = gio::ListStore::new::<gtk::FileFilter>();
                filters.append(&filter);
                let dialog = gtk::FileDialog::builder().title("Add alpha masks").modal(true).filters(&filters).default_filter(&filter).build();
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
            self.list.append(&row);
        }
    }
}
