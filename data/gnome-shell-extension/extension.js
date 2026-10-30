import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import {PopupAnimation} from 'resource:///org/gnome/shell/ui/boxpointer.js';
import * as Config from 'resource:///org/gnome/shell/misc/config.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {QuickMenuToggle, SystemIndicator} from 'resource:///org/gnome/shell/ui/quickSettings.js';

const ID = 'dev.gifdeck.GifDeck';
const PATH = '/dev/gifdeck/GifDeck';
const ACTION = 'widgets-enabled';
const SHELL_MAJOR = parseInt(Config.PACKAGE_VERSION.split('.')[0]);

// The toggle never decides anything itself: `checked` always mirrors the state GifDeck reports,
// and a click asks GifDeck for the opposite state. So the two cannot drift apart.
const Toggle = GObject.registerClass(
class GifDeckToggle extends QuickMenuToggle {
    _init() {
        super._init({title: 'GifDeck', iconName: 'image-x-generic-symbolic', toggleMode: false});
        this.menu.setHeader('image-x-generic-symbolic', 'GifDeck');

        this._group = null;
        this._watch = Gio.bus_watch_name(Gio.BusType.SESSION, ID, Gio.BusNameWatcherFlags.NONE,
            () => this._attach(), () => this._detach());

        this.connect('clicked', () => {
            if (this._group)
                this._group.change_action_state(ACTION, GLib.Variant.new_boolean(!this.checked));
            else
                this._openWindow();
        });

        // The arrow: open the GifDeck window instead of a submenu.
        this.menu.connect('open-state-changed', (_m, open) => {
            if (!open)
                return;
            this._openWindow();
            this.menu.close(PopupAnimation.NONE);
            const qs = Main.panel.statusArea.quickSettings.menu;
            if (SHELL_MAJOR >= 51)
                qs.close({fadeOnly: true});
            else
                qs.close(PopupAnimation.FADE);
        });
    }

    // GifDeck appeared (or restarted): fresh action group, fresh state.
    _attach() {
        this._detach();
        this._group = Gio.DBusActionGroup.get(Gio.DBus.session, ID, PATH);
        const sync = (_g, name) => {
            if (name === undefined || name === ACTION)
                this._sync();
        };
        this._sigs = ['action-added', 'action-state-changed'].map(s => this._group.connect(s, sync));
        this._group.list_actions(); // starts the subscription; 'action-added' follows with the state
        this._sync();
    }

    _detach() {
        if (this._group)
            this._sigs.forEach(id => this._group.disconnect(id));
        this._group = null;
        this._sigs = [];
        this.checked = false; // no daemon, no widgets
    }

    _sync() {
        const on = this._group?.get_action_state(ACTION)?.get_boolean();
        if (on !== undefined && this.checked !== on)
            this.checked = on;
    }

    // Same as running `gifdeck`: starts it, or asks the running instance to show its window.
    _openWindow() {
        Gio.DesktopAppInfo.new(`${ID}.desktop`)?.launch([], null);
    }

    destroy() {
        Gio.bus_unwatch_name(this._watch);
        this._detach();
        super.destroy();
    }
});

const Indicator = GObject.registerClass(
class GifDeckIndicator extends SystemIndicator {
    _init() {
        super._init();
        this.quickSettingsItems.push(new Toggle());
    }
});

export default class GifDeckExtension extends Extension {
    enable() {
        this._ind = new Indicator();
        Main.panel.statusArea.quickSettings.addExternalIndicator(this._ind);
    }
    disable() {
        this._ind.quickSettingsItems.forEach(i => i.destroy());
        this._ind.destroy();
        this._ind = null;
    }
}
