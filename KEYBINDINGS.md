# Lyrminal key bindings

Press `?` in the app for this list.

## General

| Key | Action |
| --- | --- |
| `?` / `F1` | Show or hide the help |
| `Esc` / `F2` | Settings |
| `p` | Choose the player to follow |
| `s` | Search all providers and pick another sync |
| `L` / `F3` | Local library: add your own lyrics from a file, URL or the clipboard |
| `R` / `F5` | Fetch the lyrics again, skipping the cache |
| `U` | Updates: check for, install and restart into a new version |
| `Tab` / `Shift+Tab` | In dialogs: jump to the next / previous category |
| `q` / `Ctrl+C` | Quit |

## View

| Key | Action |
| --- | --- |
| `v` | Switch style (Spring / Bloom) |
| `m` | Switch renderer (cell / pixel). Only in terminals with kitty graphics; elsewhere it is hidden from the help and does nothing |
| `t` | Toggle translation |
| `r` | Toggle romanization |
| `b` | Toggle background vocals |
| `i` | Toggle the track header |

## Scrolling

| Key | Action |
| --- | --- |
| `j` `k` / `↓` `↑` | Scroll by a line (pauses auto-follow) |
| `PgDn` `PgUp` / `Ctrl+D` `Ctrl+U` | Scroll by half a screen |
| `g` `G` / `Home` `End` | Jump to the top / bottom |
| `f` | Follow the song again |

## Timing

| Key | Action |
| --- | --- |
| `[` `]` | Shift this track's lyrics by ∓50 ms |
| `{` `}` | Shift this track's lyrics by ∓250 ms |
| `=` | Reset this track's offset |

## Playback

These need Settings → Controls → Playback controls, which is on by default. Turn it off to keep Lyrminal read-only.

| Key | Action |
| --- | --- |
| `Space` | Play / pause |
| `h` `l` / `←` `→` | Seek backward / forward |
| `n` `N` | Next / previous track |
| `Enter` | While scrolling, jump to the line under the cursor |
| Click the progress bar | Seek; drag along it to scrub |
| Click a lyric | Jump to that line or word |

## Mouse

The mouse wheel scrolls, a click selects, and a second click opens. Clicking outside a dialog closes it. Turn off Settings → Scrolling → Mouse if you'd rather keep your terminal's own text selection.
