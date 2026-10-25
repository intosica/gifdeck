//! "Presets" page: manage presets, and assign them to weekdays / times of day.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::ctx::AppCtx;
use crate::model::{Rule, DAY_NAMES};

pub struct PresetsPage {
    ctx: Rc<AppCtx>,
    pub root: gtk::ScrolledWindow,
    preset_list: gtk::ListBox,
    rule_list: gtk::ListBox,
    enabled: adw::SwitchRow,
    status: adw::ActionRow,
    updating: Rc<Cell<bool>>,
}

fn flat_button(icon: &str, tip: &str) -> gtk::Button {
    gtk::Button::builder().icon_name(icon).tooltip_text(tip).valign(gtk::Align::Center).css_classes(["flat"]).build()
}

fn list_box() -> gtk::ListBox {
    gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build()
}

fn clear(list: &gtk::ListBox) {
    while let Some(c) = list.first_child() {
        list.remove(&c);
    }
}

impl PresetsPage {
    pub fn new(ctx: &Rc<AppCtx>) -> Rc<PresetsPage> {
        let new_btn = gtk::Button::builder()
            .child(&adw::ButtonContent::builder().icon_name("list-add-symbolic").label("New").build())
            .build();
        let preset_list = list_box();
        let presets_group = adw::PreferencesGroup::builder()
            .title("Presets")
            .description("A saved set of widgets. The marked one is in use.")
            .header_suffix(&new_btn)
            .build();
        presets_group.add(&preset_list);

        let enabled = adw::SwitchRow::builder()
            .title("Switch presets automatically")
            .subtitle("Uses the rules below. The first matching rule wins.")
            .build();
        let status = adw::ActionRow::builder().title("Right now").build();
        let h24 = adw::SwitchRow::builder()
            .title("24-hour time")
            .subtitle("Off shows AM / PM")
            .active(ctx.state.borrow().time_24h)
            .build();
        {
            let ctx = ctx.clone();
            h24.connect_active_notify(move |s| ctx.set_time_24h(s.is_active()));
        }
        let add_rule = gtk::Button::builder()
            .child(&adw::ButtonContent::builder().icon_name("list-add-symbolic").label("Rule").build())
            .build();
        let rule_list = list_box();
        let schedule_group = adw::PreferencesGroup::builder().title("Schedule").header_suffix(&add_rule).build();
        schedule_group.add(&enabled);
        schedule_group.add(&h24);
        schedule_group.add(&status);
        let rules_group = adw::PreferencesGroup::new();
        rules_group.add(&rule_list);

        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).margin_top(12).margin_bottom(18).margin_start(12).margin_end(12).build();
        content.append(&presets_group);
        content.append(&schedule_group);
        content.append(&rules_group);
        let clamp = adw::Clamp::builder().maximum_size(640).child(&content).build();
        let root = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&clamp).build();

        let page = Rc::new(PresetsPage {
            ctx: ctx.clone(),
            root,
            preset_list,
            rule_list,
            enabled,
            status,
            updating: Rc::new(Cell::new(false)),
        });

        {
            let ctx = ctx.clone();
            new_btn.connect_clicked(move |_| ctx.new_preset());
        }
        {
            let ctx = ctx.clone();
            add_rule.connect_clicked(move |_| ctx.add_rule());
        }
        {
            let (ctx, updating) = (ctx.clone(), page.updating.clone());
            page.enabled.connect_active_notify(move |s| {
                if !updating.get() {
                    ctx.set_schedule_enabled(s.is_active());
                }
            });
        }

        page.rebuild_presets();
        page.rebuild_rules();
        page.update_status();
        page
    }

    pub fn rebuild_presets(self: &Rc<Self>) {
        self.updating.set(true);
        clear(&self.preset_list);
        let (presets, active) = {
            let st = self.ctx.state.borrow();
            (st.presets.clone(), st.active.clone())
        };
        let only_one = presets.len() <= 1;
        let mut leader: Option<gtk::CheckButton> = None;
        for p in &presets {
            let row = adw::EntryRow::builder().title("Name").text(&p.name).build();
            let radio = gtk::CheckButton::builder().valign(gtk::Align::Center).tooltip_text("Use this preset").build();
            match &leader {
                Some(l) => radio.set_group(Some(l)),
                None => leader = Some(radio.clone()),
            }
            radio.set_active(p.id == active);
            row.add_prefix(&radio);
            let dup = flat_button("edit-copy-symbolic", "Duplicate preset");
            let del = flat_button("user-trash-symbolic", "Delete preset");
            del.set_sensitive(!only_one);
            row.add_suffix(&dup);
            row.add_suffix(&del);

            {
                let (ctx, id, updating) = (self.ctx.clone(), p.id.clone(), self.updating.clone());
                radio.connect_toggled(move |r| {
                    if r.is_active() && !updating.get() {
                        ctx.activate_preset(&id);
                    }
                });
            }
            {
                let (ctx, id, updating) = (self.ctx.clone(), p.id.clone(), self.updating.clone());
                row.connect_changed(move |r| {
                    if !updating.get() {
                        ctx.rename_preset(&id, r.text().as_str());
                    }
                });
            }
            {
                let (ctx, id) = (self.ctx.clone(), p.id.clone());
                dup.connect_clicked(move |_| ctx.duplicate_preset(&id));
            }
            {
                let (ctx, id) = (self.ctx.clone(), p.id.clone());
                del.connect_clicked(move |_| ctx.delete_preset(&id));
            }
            self.preset_list.append(&row);
        }
        self.updating.set(false);
    }

    pub fn update_status(&self) {
        let (enabled, has_rules) = {
            let st = self.ctx.state.borrow();
            (st.schedule_enabled, !st.rules.is_empty())
        };
        self.updating.set(true);
        self.enabled.set_active(enabled);
        self.updating.set(false);
        let text = match (self.ctx.scheduled_now(), enabled) {
            (Some(name), true) => format!("Schedule selects “{name}”"),
            (Some(name), false) => format!("Schedule is off (would select “{name}”)"),
            (None, _) if has_rules => "No rule matches the current time".to_string(),
            (None, _) => "No rules yet".to_string(),
        };
        self.status.set_subtitle(&glib::markup_escape_text(&text));
    }

    pub fn rebuild_rules(self: &Rc<Self>) {
        self.updating.set(true);
        clear(&self.rule_list);
        let (rules, presets) = {
            let st = self.ctx.state.borrow();
            (st.rules.clone(), st.presets.iter().map(|p| (p.id.clone(), p.name.clone())).collect::<Vec<_>>())
        };
        let total = rules.len();
        for (i, rule) in rules.iter().enumerate() {
            self.rule_list.append(&self.build_rule(rule, i, total, &presets));
        }
        self.updating.set(false);
        self.update_status();
    }

    /// One compact row per rule: preset, days, time window, order and delete.
    fn build_rule(self: &Rc<Self>, rule: &Rule, idx: usize, total: usize, presets: &[(String, String)]) -> gtk::ListBoxRow {
        let ctx = self.ctx.clone();
        let id = rule.id.clone();
        let h24 = ctx.state.borrow().time_24h;
        let wrap = adw::WrapBox::builder().child_spacing(8).line_spacing(6).margin_top(8).margin_bottom(8).margin_start(12).margin_end(12).build();
        let edit = {
            let (ctx, id) = (ctx.clone(), id.clone());
            Rc::new(move |f: Box<dyn FnOnce(&mut Rule)>| ctx.edit_rule(&id, f))
        };
        let center = |w: &gtk::Widget| w.set_valign(gtk::Align::Center);

        // preset
        let names: Vec<&str> = presets.iter().map(|(_, n)| n.as_str()).collect();
        let selected = presets.iter().position(|(i, _)| *i == rule.preset).unwrap_or(0) as u32;
        let preset_row = gtk::DropDown::from_strings(&names);
        preset_row.set_selected(selected);
        center(preset_row.upcast_ref());
        {
            let (edit, updating) = (edit.clone(), self.updating.clone());
            let ids: Vec<String> = presets.iter().map(|(i, _)| i.clone()).collect();
            preset_row.connect_selected_notify(move |r| {
                if updating.get() {
                    return;
                }
                if let Some(pid) = ids.get(r.selected() as usize).cloned() {
                    edit(Box::new(move |rule| rule.preset = pid));
                }
            });
        }
        wrap.append(&preset_row);

        // days
        let days_box = gtk::Box::builder().valign(gtk::Align::Center).css_classes(["linked"]).build();
        for (d, label) in DAY_NAMES.iter().enumerate() {
            let t = gtk::ToggleButton::builder().label(&label[..1]).tooltip_text(*label).active(rule.days[d]).build();
            let (edit, updating) = (edit.clone(), self.updating.clone());
            t.connect_toggled(move |t| {
                if updating.get() {
                    return;
                }
                let on = t.is_active();
                edit(Box::new(move |rule| rule.days[d] = on));
            });
            days_box.append(&t);
        }
        wrap.append(&days_box);

        // time window
        let all_day = gtk::CheckButton::builder().label("All day").active(rule.all_day).valign(gtk::Align::Center).build();
        let window = gtk::Box::builder().spacing(6).valign(gtk::Align::Center).visible(!rule.all_day).build();
        {
            let edit = edit.clone();
            window.append(&super::time_box(rule.start, h24, move |m| edit(Box::new(move |r| r.start = m))));
        }
        window.append(&gtk::Label::new(Some("–")));
        {
            let edit = edit.clone();
            window.append(&super::time_box(rule.end, h24, move |m| edit(Box::new(move |r| r.end = m))));
        }
        window.set_tooltip_text(Some("An end earlier than the start means after midnight"));
        {
            let (edit, updating, window) = (edit.clone(), self.updating.clone(), window.clone());
            all_day.connect_toggled(move |s| {
                let on = s.is_active();
                window.set_visible(!on);
                if !updating.get() {
                    edit(Box::new(move |r| r.all_day = on));
                }
            });
        }
        wrap.append(&all_day);
        wrap.append(&window);

        // order + delete
        let up = flat_button("go-up-symbolic", "Check this rule earlier");
        let down = flat_button("go-down-symbolic", "Check this rule later");
        let del = flat_button("user-trash-symbolic", "Delete rule");
        up.set_sensitive(idx > 0);
        down.set_sensitive(idx + 1 < total);
        {
            let (ctx, id) = (ctx.clone(), id.clone());
            up.connect_clicked(move |_| ctx.move_rule(&id, -1));
        }
        {
            let (ctx, id) = (ctx.clone(), id.clone());
            down.connect_clicked(move |_| ctx.move_rule(&id, 1));
        }
        {
            let (ctx, id) = (ctx.clone(), id.clone());
            del.connect_clicked(move |_| ctx.remove_rule(&id));
        }
        for b in [&up, &down, &del] {
            wrap.append(b);
        }
        gtk::ListBoxRow::builder().activatable(false).child(&wrap).build()
    }
}
