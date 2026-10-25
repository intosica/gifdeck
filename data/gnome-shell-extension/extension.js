import Gio from 'gi://Gio';
import GObject from 'gi://GObject';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {QuickToggle, SystemIndicator} from 'resource:///org/gnome/shell/ui/quickSettings.js';

const ID = 'dev.gifdeck.GifDeck';
const PATH = '/dev/gifdeck/GifDeck';

const Toggle = GObject.registerClass(
class GifDeckToggle extends QuickToggle {
    _init() {
        super._init({title: 'GifDeck', iconName: 'image-x-generic-symbolic', toggleMode: true});
        this._group = Gio.DBusActionGroup.get(Gio.DBus.session, ID, PATH);
        this._group.list_actions(); // subscribe to state changes
        this._sig = this._group.connect('action-state-changed', (_g, name, v) => {
            if (name === 'widgets-enabled') this._sync(v.get_boolean());
        });
        this.connect('clicked', () =>
            this._group.activate_action('widgets-enabled', null));
        this._sync(this._group.get_action_state('widgets-enabled')?.get_boolean() ?? true);
    }
    _sync(on) { if (this.checked !== on) this.checked = on; }
    destroy() { this._group.disconnect(this._sig); super.destroy(); }
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
