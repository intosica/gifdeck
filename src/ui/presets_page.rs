//! "Presets" page: manage presets, and assign them to weekdays / times of day.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
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
    expanded: RefCell<HashSet<String>>,
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

/// "hh : mm" editor returning minutes since midnight.
fn time_box(minutes: u16, on_change: impl Fn(u16) + 'static) -> gtk::Box {
    let hours = gtk::SpinButton::with_range(0.0, 23.0, 1.0);
    let mins = gtk::SpinButton::with_range(0.0, 59.0, 1.0);
    hours.set_value((minutes / 60) as f64);
    mins.set_value((minutes % 60) as f64);
    for s in [&hours, &mins] {
        s.set_valign(gtk::Align::Center);
        s.set_wrap(true);
        s.set_width_chars(2);
        s.connect_output(|s| {
            s.set_text(&format!("{:02}", s.value() as i32));
            glib::Propagation::Stop
        });
    }
    let cb = Rc::new(on_change);
    for s in [&hours, &mins] {
        let (cb, hours, mins) = (cb.clone(), hours.clone(), mins.clone());
        s.connect_value_changed(move |_| cb((hours.value() as u16) * 60 + mins.value() as u16));
    }
    let b = gtk::Box::builder().spacing(4).valign(gtk::Align::Center).build();
    b.append(&hours);
    b.append(&gtk::Label::new(Some(":")));
    b.append(&mins);
    b
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
        let add_rule = gtk::Button::builder()
            .child(&adw::ButtonContent::builder().icon_name("list-add-symbolic").label("Rule").build())
            .build();
        let rule_list = list_box();
        let schedule_group = adw::PreferencesGroup::builder().title("Schedule").header_suffix(&add_rule).build();
        schedule_group.add(&enabled);
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
            expanded: RefCell::new(HashSet::new()),
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

    fn build_rule(self: &Rc<Self>, rule: &Rule, idx: usize, total: usize, presets: &[(String, String)]) -> adw::ExpanderRow {
        let ctx = self.ctx.clone();
        let id = rule.id.clone();
        let name_of = |pid: &str| presets.iter().find(|(i, _)| i == pid).map(|(_, n)| n.clone()).unwrap_or_default();

        let row = adw::ExpanderRow::builder()
            .title(glib::markup_escape_text(&format!("Use “{}”", name_of(&rule.preset))))
            .subtitle(glib::markup_escape_text(&rule.summary()))
            .expanded(self.expanded.borrow().contains(&id))
            .build();
        {
            let (this, id) = (Rc::downgrade(self), id.clone());
            row.connect_expanded_notify(move |r| {
                if let Some(this) = this.upgrade() {
                    if r.is_expanded() {
                        this.expanded.borrow_mut().insert(id.clone());
                    } else {
                        this.expanded.borrow_mut().remove(&id);
                    }
                }
            });
        }

        let up = flat_button("go-up-symbolic", "Check this rule earlier");
        let down = flat_button("go-down-symbolic", "Check this rule later");
        let del = flat_button("user-trash-symbolic", "Delete rule");
        up.set_sensitive(idx > 0);
        down.set_sensitive(idx + 1 < total);
        row.add_suffix(&del);
        row.add_suffix(&down);
        row.add_suffix(&up);
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

        // Keep the header in sync with the edited rule without rebuilding (rebuilding would close the row).
        let refresh_header: Rc<dyn Fn()> = {
            let (row, ctx, id) = (row.downgrade(), ctx.clone(), id.clone());
            Rc::new(move || {
                let Some(row) = row.upgrade() else { return };
                let st = ctx.state.borrow();
                if let Some(r) = st.rules.iter().find(|r| r.id == id) {
                    let name = st.preset(&r.preset).map(|p| p.name.clone()).unwrap_or_default();
                    row.set_title(&glib::markup_escape_text(&format!("Use “{name}”")));
                    row.set_subtitle(&glib::markup_escape_text(&r.summary()));
                }
            })
        };
        let edit = {
            let (ctx, id, refresh_header) = (ctx.clone(), id.clone(), refresh_header.clone());
            Rc::new(move |f: Box<dyn FnOnce(&mut Rule)>| {
                ctx.edit_rule(&id, f);
                refresh_header();
            })
        };

        // preset
        let names: Vec<&str> = presets.iter().map(|(_, n)| n.as_str()).collect();
        let selected = presets.iter().position(|(i, _)| *i == rule.preset).unwrap_or(0) as u32;
        let preset_row = adw::ComboRow::builder().title("Preset").model(&gtk::StringList::new(&names)).selected(selected).build();
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
        row.add_row(&preset_row);

        // days
        let days_row = adw::ActionRow::builder().title("Days").build();
        let days_box = gtk::Box::builder().spacing(4).valign(gtk::Align::Center).css_classes(["linked"]).build();
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
        days_row.add_suffix(&days_box);
        row.add_row(&days_row);

        // time window
        let all_day = adw::SwitchRow::builder().title("All day").active(rule.all_day).build();
        let from = adw::ActionRow::builder().title("From").build();
        let until = adw::ActionRow::builder().title("Until").subtitle("An earlier time than “From” means after midnight").build();
        {
            let edit = edit.clone();
            from.add_suffix(&time_box(rule.start, move |m| edit(Box::new(move |r| r.start = m))));
        }
        {
            let edit = edit.clone();
            until.add_suffix(&time_box(rule.end, move |m| edit(Box::new(move |r| r.end = m))));
        }
        from.set_sensitive(!rule.all_day);
        until.set_sensitive(!rule.all_day);
        {
            let (edit, updating, from, until) = (edit.clone(), self.updating.clone(), from.clone(), until.clone());
            all_day.connect_active_notify(move |s| {
                let on = s.is_active();
                from.set_sensitive(!on);
                until.set_sensitive(!on);
                if !updating.get() {
                    edit(Box::new(move |r| r.all_day = on));
                }
            });
        }
        row.add_row(&all_day);
        row.add_row(&from);
        row.add_row(&until);
        row
    }
}
