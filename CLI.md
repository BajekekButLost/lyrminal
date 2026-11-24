# Command line

Run `lyrminal` with no command to start the app. The commands below change Lyrminal from a shell or a script, and `lyrminal <command> --help` shows the options of each one.

- [Starting the app](#starting-the-app)
- [Global options](#global-options)
- [Reading from stdin](#reading-from-stdin)
- [Setting keys](#setting-keys)
- [`lyrminal paths`](#lyrminal-paths)
- [`lyrminal settings`](#lyrminal-settings)
- [`lyrminal providers`](#lyrminal-providers)
- [`lyrminal library`](#lyrminal-library)
- [`lyrminal now-playing`](#lyrminal-now-playing)
- [`lyrminal update`](#lyrminal-update)
- [Talking to the running app](#talking-to-the-running-app)

## Starting the app

```text
lyrminal [global options] [temporary settings]
```

### Temporary settings

These change a setting for one session only. The app shows such a setting with a 🔒 in Settings and won't let you change it. It is never written to the config file: when the app saves, the file keeps its own value, and a live reload or a `lyrminal settings` command doesn't override it either.

| Option | What it does |
| --- | --- |
| `--set <KEY=VALUE>` | Set any [setting key](#setting-keys), e.g. `--set appearance.style=bloom`. Repeat it for more. `--set providers.order=lrclib,spicy-lyrics` sets the provider order. |
| `--renderer <cell\|pixel>` | Use this renderer |
| `--style <spring\|bloom>` | Use this animation style |
| `--preset <lyrminal\|spicy-lyrics>` | Use this look preset. Every setting the preset changes is locked |
| `--player <NAME>` | Only follow this player: a process name, MPRIS bus name or player identity |
| `--offset <MS>` | Use this global sync offset. Positive shows lines earlier |
| `--no-controls` | Stay read-only: no play, pause, seek or skip |

Temporary settings only work when starting the app, not with a command. When the in-app updater restarts Lyrminal, they are passed on to the new version.

## Global options

These work for the app and for every command, and can go before or after the command.

| Option | What it does |
| --- | --- |
| `--base-directory <DIR>` | Use `DIR` as the home folder for Lyrminal's files (`DIR/.config/lyrminal`, `DIR/.cache/lyrminal` and so on). It is created if it doesn't exist |
| `--temp-base-directory` | The same, in a new randomly named folder in the system temp directory. Its path is printed when you quit, and it is not deleted |
| `--config <FILE>` | Read and save the settings in `FILE` instead of `config.json` |
| `--state-file <FILE>` | Keep per-track offsets, chosen syncs and the tour status in `FILE` instead of `state.json` |
| `--cache-dir <DIR>` | Keep cached lyrics and lookups in `DIR` |
| `--library-dir <DIR>` | Use `DIR` as the local library (its `index.json` and the saved syncs) |
| `--socket <FILE>` | The socket the running app listens on (see [Talking to the running app](#talking-to-the-running-app)) |
| `-h`, `--help` | Show the help |
| `-V`, `--version` | Show the version |

A path option wins over `--base-directory` for that one path. `~` means your home folder, and relative paths start in the current folder.

## Reading from stdin

Every command that changes something can take its value from stdin instead of an argument. Leave the value out, or give `-`:

```sh
echo bloom | lyrminal settings set appearance.style
pass show spicy-key | lyrminal providers set spicy-lyrics apiKey
printf 'lrclib\nspicy-lyrics\n' | lyrminal providers order
lyrminal library list | grep Kendrick | cut -c1-8 | lyrminal library remove
curl -s https://example.com/song.lrc | lyrminal library add
```

A single value is read whole, without the trailing newline. Lists (setting keys, provider ids, sync ids) take one item per line.

## Setting keys

Settings are named by their place in `config.json`, such as `appearance.fontScale` or `sync.offsetMs`. Case, `-` and `_` don't matter, so `appearance.font-scale` works too. `lyrminal settings list` shows every key, and `lyrminal settings info <key>` explains one.

Values are checked the same way as in the settings screen:

- **On/off settings** take `on`, `off`, `true`, `false`, `yes`, `no`, `1` or `0`.
- **Choices** take the names shown by `settings list`, e.g. `cell`, `pixel` or `fixed-process`. The start of a name is enough when it's unambiguous.
- **Numbers** must be inside the setting's range. A unit after the number is allowed (`250ms`).
- **Text** is taken as typed. `player.preferred` is a comma-separated list.

`appearance.preset` is a key of its own that applies a look preset (`lyrminal` or `spicy-lyrics`). It reads `custom` when the look matches neither.

## `lyrminal paths`

```text
lyrminal paths [config|state|cache|library|socket]
```

Prints where Lyrminal keeps its files, after any path options are applied. Give one name to print only that path, e.g. `cd "$(lyrminal paths library)"`.

## `lyrminal settings`

Covers every setting except the lyrics providers, which have [their own commands](#lyrminal-providers).

| Command | What it does |
| --- | --- |
| `settings list [TAB] [--json] [--reveal]` | Every setting grouped like the settings tabs, with its key and value. Values that differ from the default show the default too. `TAB` limits it to one tab, e.g. `appearance` |
| `settings get <KEY> [--reveal]` | Print one value |
| `settings set <KEY> [VALUE]` | Change a setting |
| `settings toggle [KEY…]` | Turn on/off settings the other way |
| `settings reset [KEY…]` | Put settings back to their defaults |
| `settings reset --all` | Reset every setting except the provider settings |
| `settings info <KEY> [--json]` | What a setting does, the values it takes, its value and its default |
| `settings path` | Print where the config file is |
| `settings edit` | Open the config file in `$VISUAL` or `$EDITOR` (or `vi`), then check it. The file is created with the defaults if it doesn't exist yet |

`--reveal` shows secret values such as API keys, which are masked otherwise.

```sh
lyrminal settings set sync.offsetMs -120
lyrminal settings toggle lyrics.translation lyrics.romanization
lyrminal settings set appearance.preset spicy-lyrics
lyrminal settings list appearance --json
```

## `lyrminal providers`

Providers are named by id: `local-library`, `spicy-lyrics`, `lrc-red`, `unison`, `lrcmux` and `lrclib`. Their names (`LRCLIB`, `lrc.red`, `Better Lyrics`) and any unambiguous start (`spicy`, `local`) work too.

| Command | What it does |
| --- | --- |
| `providers list [--json]` | The providers in priority order, and whether each is on |
| `providers enable [ID…]` | Turn providers on |
| `providers disable [ID…]` | Turn providers off |
| `providers move <ID> [PLACE]` | Change one provider's priority. `PLACE` is a position (1 is tried first), `up`, `down`, `top` or `bottom` |
| `providers order [ID…]` | Set the whole order. Providers you leave out go back to their default position |
| `providers options <ID> [--json] [--reveal]` | A provider's options and their values |
| `providers get <ID> <OPTION> [--reveal]` | Print one option |
| `providers set <ID> <OPTION> [VALUE]` | Change one option |
| `providers reset [ID…]` | Put providers back to their default options, position and on/off |
| `providers reset --all` | Reset every provider setting, including the order and Prefer synced lyrics |
| `providers prefer-synced [on\|off\|toggle]` | Show or change "Prefer synced lyrics": on picks the best sync, off takes the first provider with any lyrics |

Options of each provider (case, `-` and `_` don't matter here either):

| Provider | Options |
| --- | --- |
| `local-library` | `enabled`, `prioritize`, `defaultDir`, `durationToleranceSecs` |
| `spicy-lyrics` | `enabled`, `useOwnKey`, `apiKey`, `minTier`, `checkResults`, `checkedResults` |
| `lrc-red`, `unison`, `lrcmux`, `lrclib` | `enabled`, `durationToleranceSecs` |

As setting keys these are `providers.<provider>.<option>`, e.g. `providers.spicyLyrics.minTier`, which is what `--set` takes.

```sh
lyrminal providers move lrclib top
lyrminal providers set spicy-lyrics min-tier likely
lyrminal providers disable unison lrcmux
```

## `lyrminal library`

Syncs are named by their id. `library list` shows the first 8 characters, and any unambiguous start of an id works.

| Command | What it does |
| --- | --- |
| `library list [-s TEXT] [--json]` | The saved syncs. `-s` only lists those whose title, artist or album contains `TEXT` |
| `library show <ID> [--json]` | Everything about a sync: song, source, format, sync type, where its saved copy is and when it was added |
| `library show <ID> --content` | Print the saved copy of the lyrics file |
| `library add [FILE\|URL\|-]` | Save a sync. Without a file or URL, the lyrics are read from stdin |
| `library update <ID>` | Change a saved sync |
| `library remove [ID…] [-y]` | Remove syncs. In a terminal it asks first, unless you pass `-y` |
| `library use [ID]` | Use a saved sync for the song the running app is playing, like Enter on its card |

### Adding a sync

By default `library add` saves the sync for the song the running app is playing. To add it for another song, or when the app isn't running, describe the song:

| Option | What it does |
| --- | --- |
| `--title <TITLE>` | The song's title. Giving it means the playing song isn't used |
| `--artist <NAME>` | An artist. Repeat it for more |
| `--album <ALBUM>` | The album |
| `--duration <LENGTH>` | The length, in seconds or as `m:ss` |
| `--isrc <ISRC>` | The ISRC |
| `--spotify-id <ID>` | The Spotify track id |
| `--format <FORMAT>` | The format instead of detecting it: `lrc`, `ttml`, `srt`, `vtt`, `ass`, `sbv`, `sami`, `json` or `text` |
| `--no-watch` | Don't re-read the file or URL every time the sync loads ("Check for changes" is on otherwise) |
| `--temporary` | Don't save it: show it in the running app for the playing song until the song changes |

The other options can also be given together with the playing song, and then replace that part of it, e.g. `--album` to fix a missing album. A sync for a song that already has one replaces it, the same as in the app.

```sh
lyrminal library add ~/syncs/song.ttml
lyrminal library add https://example.com/lyrics.lrc --title "Song" --artist "Artist" --duration 3:45
xclip -o | lyrminal library add --temporary
```

### Changing a sync

`library update <ID>` takes the same song options as `add`, plus:

| Option | What it does |
| --- | --- |
| `--source <FILE\|URL\|->` | Take the lyrics from this file or URL from now on. `-` reads new lyrics from stdin |
| `--format <FORMAT>` | The format, or `auto` to detect it |
| `--watch <on\|off>` | Whether to re-read the file or URL every time the sync loads |
| `--refresh` | Read the file or URL again now and update the saved copy |

Changing the song (title, artist, Spotify id and so on) gives the sync a new id.

When the app is running, it is told about every change to the library and loads the playing song's lyrics again if the change affects them.

## `lyrminal now-playing`

Shows what the running app is playing and where it is in the lyrics. The position is worked out when you ask, with the global and per-track offsets applied.

```text
$ lyrminal now-playing
playing · Childish Gambino · III. Telegraph Ave. · 1:23/4:47 · Oakland, Oakland
```

| Option | What it does |
| --- | --- |
| `-s`, `--separator <TEXT>` | Put `TEXT` between the fields instead of ` · ` |
| `-f`, `--fields <FIELD,…>` | Print these fields, in this order |
| `--full` | Print every field: status, artist, title, album, position, length, line, lines, text, progress, word, sync and provider |
| `--raw` | Print unformatted values (seconds, progress from 0 to 1) separated by tabs |
| `--json` | Print a JSON object with every field, or only those given with `--fields` |
| `--strict` | Print nothing and exit with code 3 when nothing is playing or there are no lyrics |
| `-w`, `--watch` | Keep running and print a new line whenever the track, status, line or word changes. While the app isn't running it prints an empty line and waits for it |
| `--interval <MS>` | With `--watch`, also print every `MS` milliseconds, e.g. to keep the position up to date |

Fields:

| Field | What it is |
| --- | --- |
| `status` | `playing`, `paused`, `stopped` or `no player` |
| `player` | The player's name |
| `artist`, `title`, `album` | The song |
| `time` | Position and length together, `1:23/4:47` |
| `position`, `length` | The position and the song's length |
| `line`, `lines` | The number of the line being sung and how many lines there are. `line` is 0 before the first line |
| `text` | The line being sung. Empty during an instrumental break |
| `progress` | How far into that line the song is |
| `word` | The word being sung, for word-synced lyrics |
| `next` | The next line |
| `sync` | `word`, `line` or `unsynced` |
| `provider`, `source` | Where the lyrics came from |
| `lyrics` | `ready`, `loading`, `not found`, `no track`, `unsupported` or `no player` |

The JSON output also has `artists` (a list), `interlude` (true during a break), `offset` (seconds), `spotifyId` and `isrc`. Without `--strict`, an app with nothing playing prints the status with the other fields empty. When the app isn't running, `now-playing` exits with code 1.

A waybar module, for example:

```json
"custom/lyrics": {
    "exec": "lyrminal now-playing --watch --fields text",
    "format": "{}"
}
```

## `lyrminal update`

Checks GitHub for a newer release and installs it over the running binary, the same way as the in-app updater. The download is checked with `--version` before it replaces anything. It asks before installing.

| Option | What it does |
| --- | --- |
| `-y`, `--yes` | Install without asking. Needed when stdin isn't a terminal |
| `--check` | Only check. Exits with 0 when up to date, 1 when a new release is out and 2 when the check fails |
| `--from-source` | Build the release on this machine with `cargo` instead of downloading the prebuilt binary. Needs `cargo` and `tar`. Used automatically when there's no prebuilt binary for your system |
| `--version <vX.Y.Z>` | Install this release instead of the latest, even an older one |
| `--force` | Install even when that version is already installed |

Builds running from a `cargo build` `target/` folder don't update themselves. A running Lyrminal keeps the old version until you restart it.

## Talking to the running app

While the app runs, it listens on a socket. By default this is `$XDG_RUNTIME_DIR/lyrminal.sock`, or `lyrminal.sock` in the state folder when there's no runtime folder or a base directory is in use. `now-playing`, `library use` and `library add --temporary` ask the app through it. After `settings`, `providers` and `library` commands, the app is told to reload, so changes show up right away. Use `--socket`, or the same `--base-directory`, to talk to an app started with another one.

When a second Lyrminal starts while one is running, the first keeps the socket.

The app also reloads `config.json` on its own when the file changes, for example after you save it in an editor. Turn that off in Settings → About → Live reload config (`general.liveReload`). A file with a mistake is ignored, and the app keeps its current settings until you fix it.
