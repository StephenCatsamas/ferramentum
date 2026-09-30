# Kai Window Focus

Internal status: **beta**. This companion extension supplies GNOME Wayland window activation
for `kai status --watch`. It exports one narrow D-Bus method; it does not enable Shell.Eval.
The code targets GNOME Shell 45–51's ES module API. Native GNOME testing is still required;
the activation logic has fixture tests.

From this directory, package and install it for your user:

```sh
gnome-extensions pack --force --extra-source=focus.js --out-dir=/tmp .
gnome-extensions install --force /tmp/kai-window-focus@ferramentum.shell-extension.zip
```

Log out and back in on Wayland, then enable it:

```sh
gnome-extensions enable kai-window-focus@ferramentum
```

The dashboard also needs `gdbus` (usually provided by GLib). The extension refuses ambiguous
shared terminal processes and stale process IDs, and does not activate windows while locked.
Use a separate Foot, Alacritty, xterm, st, or urxvt process per window. GNOME Terminal, Console,
and other terminals with internal tabs need a further terminal integration; this extension
alone cannot identify their tabs. GNOME on X11 can use the `wmctrl`/`xprop` backend instead.

Run fixture tests with `node --test focus.test.js`. Remove with
`gnome-extensions uninstall kai-window-focus@ferramentum`.
