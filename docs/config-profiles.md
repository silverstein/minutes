# Config saves and isolated voice testing

Minutes normally reads `~/.config/minutes/config.toml` (or the equivalent
under `XDG_CONFIG_HOME`). `MINUTES_CONFIG_PATH` overrides only the Minutes
config path for that process and its child Minutes processes. Empty values
retain the normal path. It does not move recordings, meeting memory, or
session logs, and does not change other applications' XDG settings.

For a voice dogfood wrapper, set an absolute path before launching `talk`:

```sh
export MINUTES_CONFIG_PATH="$HOME/.config/minutes-voice/config.toml"
exec /path/to/current/minutes talk "$@"
```

Create that profile deliberately from the current desktop settings and the
desired voice settings. Keep its directory private and its file mode 0600.
Continue loading API credentials through the existing environment/secret
store; do not put a key in the profile. This is a separate snapshot: later
desktop preference changes are not automatically copied into it.

Full config saves preserve unknown root keys, tables, nested table fields,
comments, and unchanged array values. Known settings are updated, and known
optional settings explicitly cleared by the caller are removed. Edited arrays
are replaced as units; the saver does not guess which old element corresponds
to a new one. Malformed or incompatible existing files are not overwritten.
Saves use a same-directory temporary file and atomic replacement, and reject
a detected intervening write. This is not a cross-process transaction protocol;
simultaneous saves from separate apps should still be avoided.

These protections apply only to rebuilt applications. An older installed app
cannot gain them from newer source code or a newer CLI. Use the signed
`~/Applications/Minutes Dev.app` installer for desktop testing, and keep the
voice profile isolated while older production builds remain installed.
