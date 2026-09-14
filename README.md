# cord.hx

Discord Rich Presence for [Helix](https://helix-editor.com). A port of
[cord.nvim](https://github.com/vyfor/cord.nvim).

The plugin is a thin Steel layer over a native core (`libcord_hx`) that keeps running across
Steel engine reloads (`:config-reload`, trusting a workspace), so the presence and its timestamp
survive them.

It requires a Helix build with Steel plugin support, such as
[steelix](https://github.com/mattwparas/helix/tree/steel-event-system).

- One shared server keeps a single Discord connection for every Helix instance.
- Idle detection hands the presence over to the most recently active instance.
- 130+ icons in several themes, plus variables, per-workspace rules, buttons and hooks.
- Built-in extensions: diagnostics, Last.fm, tmux, zellij and more.

## Installation

### forge

```sh
forge pkg install --git https://github.com/NikSneMC/cord.hx.git
```

Forge builds the `libcord_hx` dylib and the `cord` server in one cargo build. The plugin finds
the server in forge's build directory, then in `PATH`. To use a binary installed elsewhere, set
`'advanced (hash 'server (hash 'executable-path "<prefix>"))`; the plugin then runs `<prefix>/bin/cord`.

Then, in `init.scm`:

```scheme
(require "cord/cord.scm")
```

cord starts once `init.scm` has been evaluated. Configuring it is optional.

### Nix (Home Manager with nhx)

cord plugs into [nhx](https://github.com/Ra77a3l3-jar/nhx) as a regular plugin: import the module
next to nhx's and configure it under `programs.nhx.plugins.cord`:

```nix
# flake.nix
inputs.cord-hx = {
  url = "github:NikSneMC/cord.hx";
  inputs.nhx.follows = "nhx";
};
```

```nix
{
  imports = [
    inputs.nhx.homeManagerModules.default
    inputs.cord-hx.homeManagerModules.default
  ];

  programs.nhx = {
    enable = true;
    steel.enable = true;

    plugins.cord = {
      enable = true;
      config = {
        display.theme = "catppuccin";
        idle.timeout = 600000;
        timestamp.resetOnIdle = true;
        text.workspace = { __raw = "(lambda (opts) \"Hacking\")"; };
        workspaceRules = [
          { glob = "^~/Projects/[^/]+/work"; details = "Probably some work-related stuff"; state = false; }
          { glob = [ "^~/Projects/public" "^~/Projects/contrib" ]; details = "\${workspace}"; }
          { path = "~/Projects/secret-project"; visible = false; }
        ];
        extensions = {
          diagnostics.enable = true;
          localTime = { enable = true; affectIdle = false; };
        };
      };
    };
  };
}
```

The options mirror the Scheme configuration below, named in camelCase like other nhx plugin
options: `swap-fields?` is `swapFields`, `advanced.plugin.cursor-update` is
`advanced.plugin.cursorUpdate`, and enumerations are strings (`logLevel = "debug"`). Extensions
are enabled with `enable`, and their options follow the same naming
(`scopedTimestamps = { enable = true; pause = false; }`). Procedures, for text options
and `hooks`, are written as `{ __raw = "(lambda (opts) ...)"; }`.

## Configuration

```scheme
(cord-configure!
 (hash 'display (hash 'theme 'catppuccin 'flavor 'accent)
       'idle (hash 'timeout 600000)
       'text (hash 'workspace (lambda (opts) (string-append "In " (hash-ref opts 'workspace))))
       'extensions (list 'diagnostics)))
```

Every key is optional, and the configuration is merged with the defaults. Keys are kebab-case
symbols, options ending in `?` take booleans, and enumerations take symbols (strings work too).

| Option | Default | Description |
| --- | --- | --- |
| `'enabled?` | `#t` | Start cord |
| `'log-level` | `'off` | `'trace` `'debug` `'info` `'warn` `'error` `'off` |
| `'editor` | | `'client` (`'auto`, `'helix`, `'nova` for [Yazelix Nova](https://nova.yazelix.com) or a Discord application ID string; `'auto` picks Nova inside Nova, Helix otherwise), `'tooltip`, `'icon` (both default to the client's) |
| `'display` | | `'theme` (`'default` `'atom` `'catppuccin` `'minecraft` `'void` `'classic`), `'flavor` (`'dark` `'light` `'accent`), `'view` (`'full` `'editor` `'asset` `'auto`), `'swap-fields?`, `'swap-icons?` |
| `'timestamp` | | `'enabled?`, `'reset-on-idle?`, `'reset-on-change?`, `'shared?` |
| `'idle` | | `'enabled?`, `'timeout` (ms), `'show-status?`, `'ignore-focus?`, `'unidle-on-focus?`, `'smart-idle?`, `'details`, `'state`, `'tooltip`, `'icon` |
| `'text` | | `'default`, `'workspace`, `'viewing`, `'editing`, `'file-browser`, `'plugin-manager`, `'lsp`, `'docs`, `'vcs`, `'notes`, `'debug`, `'test`, `'games`, `'diagnostics`, `'terminal`, `'dashboard` |
| `'buttons` | `'()` | Up to 2 `(hash 'label ... 'url ...)` |
| `'assets` | | Hash map from Helix language name, file name or extension (strings) to an icon URL or `(hash 'icon 'tooltip 'name 'text 'type)` |
| `'variables` | | `#t` enables `${name}` placeholders, or a hash map of extra variables |
| `'workspace-rules` | `'()` | See below |
| `'hooks` | | See below |
| `'extensions` | `'()` | See below |
| `'advanced` | | `'plugin` (`'autocmds?`, `'cursor-update` `'on-hold`/`'on-move`/`'none`, `'match-in-mappings?`, `'debounce` `(hash 'delay 'interval)`), `'server` (`'pipe-path`, `'executable-path`, `'timeout`), `'discord` (`'pipe-paths`, `'reconnect`, `'sync`), `'workspace` (`'root-markers`, `'limit-to-cwd?`) |

Option values may be procedures. A procedure receives `opts`, a hash map with
`'filename`, `'filetype`, `'buftype`, `'read-only?`, `'workspace` (repository name), `'worktree` (linked git
worktree name, if any), `'workspace-dir`, `'repo-url`, `'cursor-line`, `'cursor-column` (1-based),
`'cursor-char` (0-based), `'idle?`, `'focused?`, `'timestamp`, `'buttons`, `'type`, `'icon`, `'text`,
`'name` and `'tooltip`, plus the configured variables.
Text options may also return `#t` to skip an update or `#f` to clear the activity.

### Workspace rules

Workspace rules change the activity per workspace. The first matching rule wins. A rule matches
with any combination of these, all of which have to match:

| Key | Matches |
| --- | --- |
| `'path` | the workspace directory, exactly (a string or a list) |
| `'name` | the workspace (repository) name (a string or a list) |
| `'glob` | a regex (or a list) against the buffer path, or the workspace directory for buffers without a file |
| `'predicate` | a procedure receiving `(hash 'workspace 'workspace-dir 'filename 'path)` |

A leading `~` in `'path` and `'glob` stands for the home directory. A matching rule may set:

- `'details` and `'state`: templates (`${...}` placeholders are always substituted), procedures,
  or `#f` to omit the line.
- `'visible? #f`: hide the presence while the rule matches, letting other editor instances show
  theirs. It comes back when no hiding rule matches anymore, unless it was hidden with a command.

```scheme
'workspace-rules
(list (hash 'path "~/Projects/secret" 'visible? #f)
      (hash 'glob "^~/Projects/work" 'details "Probably some work-related stuff" 'state #f)
      (hash 'glob (list "^~/Projects/public" "^~/Projects/contrib")
            'details "${workspace}"
            'state "Editing ${filename} ${cursor-line}:${cursor-column}"))
```

### Hooks

`'hooks` maps an event to a procedure, or to `(hash 'callback f 'priority n)`.

| Event | Arguments | Return value |
| --- | --- | --- |
| `'pre-activity` | `opts` | new `opts`, or anything else to keep them |
| `'post-activity` | `opts activity` | new activity, or anything else to keep it |
| `'ready`, `'buf-enter`, `'focus-gained`, `'focus-lost` | manager state: `(hash 'ready? 'paused? 'idle? 'focused?)` | ignored |
| `'workspace-change`, `'idle-enter`, `'idle-leave` | `opts` | ignored |
| `'shutdown` | none | ignored |

```scheme
'hooks (hash 'post-activity
             (lambda (opts activity)
               (hash-insert activity 'state "Hacking")))
```

Procedures run in the editor; the native core waits for their results, so keep them quick. Commands
other than `cord`, `cord-enable`, `cord-disable` and `cord-toggle` ignore their arguments, so they
can be used as hooks (`'idle-leave cord-restart`).

### API

`cord/api.scm` provides functions for procedures and hooks:

| Function | |
| --- | --- |
| `(cord-queue-update! [force])` | queue an activity update |
| `(cord-skip-update!)` | skip the next update (in `'post-activity`: the one being built) |
| `(cord-set-activity! activity [force])` | send an activity (`'details`, `'state`, `'details-url`, `'assets` with `'large-image`/`'large-url`/..., `'timestamps`, `'buttons`, `'type`, `'status-display-type`, `'idle?`) |
| `(cord-clear-activity! [force])` | clear the activity (`#t`: clear the presence completely) |
| `(cord-hide!)`, `(cord-suppress!)`, `(cord-resume!)`, `(cord-toggle!)`, `(cord-toggle-suppress!)` | control the presence |
| `(cord-pause!)`, `(cord-pause-events!)`, `(cord-resume-events!)` | stop handling events |
| `(cord-idle!)`, `(cord-force-idle!)`, `(cord-unidle!)`, `(cord-toggle-idle! [force])` | control the idle state |
| `(cord-icon name [theme flavor])` | URL of a built-in icon |
| `(cord-run command [cwd ttl])` | cached output of a command (see below) |

Manager functions called from a procedure run once it returns, before cord continues.

Procedures must not wait for processes. `cord-run` returns the last trimmed output of a command
right away (`#f` while none has finished or the last run failed) and reruns it in the background
once it is older than `ttl` seconds (default 30); the activity is updated when the output changes:

```scheme
(require "cord/api.scm")

(cord-configure!
 (hash 'idle (hash 'icon (cord-icon "keyboard" "atom"))
       'text (hash 'workspace
                   (lambda (opts)
                     (let ([branch (cord-run '("git" "branch" "--show-current")
                                             (hash-ref opts 'workspace-dir) 60)])
                       (string-append "In " (hash-ref opts 'workspace)
                                      (if branch (string-append " (" branch ")") "")))))
       'hooks (hash 'workspace-change
                    (lambda (opts)
                      (if (equal? (hash-ref opts 'workspace) "notes") (cord-hide!) (cord-resume!))))))
```

To start cord later, configure `'enabled? #f` and call `cord-configure!` again when needed.

### Extensions

```scheme
'extensions (list 'diagnostics
                  (list 'tmux (hash 'interval 5000))
                  my-extension)
```

Built-in extensions:

- `'diagnostics`: appends the problem count.
- `'lastfm`: shows the track playing on Last.fm.
- `'local-time`: uses the local time of day as the timestamp.
- `'persistent-timer`: accumulates time per workspace across sessions.
- `'scoped-timestamps`: keeps a separate timestamp per buffer or workspace.
- `'resolver`: detects NestJS projects.
- `'tmux` and `'zellij`: hide the presence while the session is detached, and show commands
  focused next to Helix in the same window or tab (`lazygit`, `yazi`, `man`, ...). Using such a
  command is invisible to Helix: once another editor instance has taken the presence over,
  returning to the command shows it again only after it changes, is reopened or the session is
  attached again.

Their options are documented at the top of each file in `src/plugin/extensions/` (`multiplexer.rs`
for tmux and zellij). Icons for languages, file names and commands are listed in
`src/plugin/mappings.tsv`.

A custom extension is a hash map with `'name` and optional `'variables`, `'hooks`, `'assets`
and `'config`. It may instead have a `'setup` procedure that takes the configuration and
returns the extension.

### Icons

Icons are served from [NikSneMC/cord-icons](https://github.com/NikSneMC/cord-icons), a fork of
[vyfor/icons](https://github.com/vyfor/icons) (the icons of cord.nvim) that adds Helix and Yazelix
Nova and drops the Neovim ones.

## Coming from cord.nvim

Most cord.nvim configurations translate directly:

- Lua tables become hash maps with kebab-case symbols, and booleans end in `?`:
  `display = { swap_fields = true }` is `'display (hash 'swap-fields? #t)`.
- `function(opts) ... end` is `(lambda (opts) ...)`, and `opts.filename` is
  `(hash-ref opts 'filename)`.
- `extensions = { diagnostics = { scope = 'workspace' } }` is
  `'extensions (list (list 'diagnostics (hash 'scope 'workspace)))`, with the same extension
  names. The diagnostics `severity` is a symbol (`'error`), not a `{ min = ... }` table.
- The manager functions (`queue_update`, `set_activity`, `skip_update`, `hide`, `idle`, ...) are
  in `cord/api.scm` as `cord-queue-update!`, `cord-set-activity!`, and so on.
- Shell calls such as `vim.fn.system` must not block: use `cord-run`. `vim.version()` becomes
  `(cord-run '("hx" "--version"))`.
- Helix has no Neovim plugin buffers (Telescope, lazy.nvim, Mason, ...) and no terminal buffers, so
  `'buftype` is always empty. Tools running in panes next to Helix are reported by the `'tmux` and
  `'zellij` extensions instead.

The `visibility` plugin is replaced by workspace rules with `'visible? #f`. A blacklist is a list
of hiding rules:

```scheme
'workspace-rules (list (hash 'glob (list "^~/Projects/work" "^~/Projects/secret") 'visible? #f))
```

A whitelist relies on the first matching rule winning: rules for the allowed workspaces come
first, and a rule matching everything hides the rest:

```scheme
'workspace-rules (list (hash 'glob (list "^~/Projects/public" "^~/Projects/contrib"))
                       (hash 'glob "" 'visible? #f))
```

Its `action` and `fallback` procedures become `'predicate` rules, or hooks calling
`cord-hide!` and `cord-resume!`.

## Commands

| Command | Description |
| --- | --- |
| `:cord-presence-show` / `-hide` / `-suppress` / `-toggle` / `-toggle-suppress` | Control the presence |
| `:cord-idle` / `:cord-unidle` / `:cord-idle-toggle` / `:cord-idle-force` | Control the idle state |
| `:cord-enable` / `:cord-disable` / `:cord-toggle` `[idle]` | The presence, or a feature |
| `:cord-status` / `:cord-version` | Connection status and server version |
| `:cord-restart` / `:cord-reconnect` / `:cord-shutdown` | Manage the server |
| `:cord-health` / `:cord-debug` / `:cord-debug-copy` | Health and debug reports |
| `:cord <command> [args]` | The same commands, as in cord.nvim (`:cord presence hide`) |

See `keybindings-example.scm` for a keymap.

## Logging

Set `CORD_LOG_FILE` to write logs to a file, and `CORD_LOG_LEVEL` (e.g. `trace`) to override the
configured level.

## Development

- `nix develop` gives you the toolchain and `steel`.
- `cargo build --release` builds the dylib and the server.
- `cargo test --lib` runs the tests.
- `nix flake check` runs the same tests in a sandbox.

## License

Apache-2.0
