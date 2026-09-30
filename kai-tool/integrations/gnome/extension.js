// Beta companion backend. No Shell.Eval or arbitrary command execution.
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {focusWindow} from './focus.js';

const interfaceXml = `<node>
  <interface name="org.gnome.Shell.Extensions.KaiWindowFocus">
    <method name="Focus">
      <arg name="pid" type="u" direction="in"/>
      <arg name="startTicks" type="s" direction="in"/>
      <arg name="result" type="s" direction="out"/>
    </method>
  </interface>
</node>`;

export default class KaiWindowFocus extends Extension {
    enable() {
        this._service = Gio.DBusExportedObject.wrapJSObject(interfaceXml, {
            Focus: (pid, startTicks) => focusWindow({
                pid,
                startTicks,
                locked: Main.sessionMode.isLocked,
                windows: global.get_window_actors().map(actor => actor.meta_window),
                readStat: target => {
                    const [ok, data] = GLib.file_get_contents(`/proc/${target}/stat`);
                    return ok ? new TextDecoder().decode(data) : '';
                },
                activate: window => Main.activateWindow(window),
                focused: () => global.display.focus_window,
            }),
        });
        this._service.export(Gio.DBus.session, '/org/gnome/Shell/Extensions/KaiWindowFocus');
    }

    disable() {
        this._service?.unexport();
        this._service = null;
    }
}
