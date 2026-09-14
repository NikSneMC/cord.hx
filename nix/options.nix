{
  lib,
  scheme,
}: let
  inherit (lib) types mkOption concatStrings toLower stringToCharacters genAttrs optionalString mapAttrs concatLists mapAttrsToList optional;
  inherit (scheme) bool hash hashEntry render str sym;

  toKebab = name:
    concatStrings (map (c:
      if toLower c != c
      then "-${toLower c}"
      else c) (stringToCharacters name));

  raw = types.addCheck (types.attrsOf types.str) (v: v ? __raw) // {description = "Scheme expression ({ __raw = \"...\"; })";};

  leaf = type: render: description: {
    _leaf = true;
    inherit type render description;
  };
  boolOption = leaf types.bool bool;
  intOption = leaf types.int toString;
  stringOption = leaf types.str str;
  stringsOption = leaf (types.listOf types.str) render;
  enumOption = values: leaf (types.enum values) sym;
  textOption = leaf (types.oneOf [types.str types.bool raw]) render;
  templateOption = leaf (types.either types.str raw) render;
  plainKey = l: l // {plainKey = true;};

  isBoolLeaf = l: l.type.name == "bool";

  schema = {
    enabled = boolOption "Start cord.";
    logLevel = enumOption ["trace" "debug" "info" "warn" "error" "off"] "Log level.";

    editor = {
      client = stringOption "`auto` (default: `nova` inside Yazelix Nova, `helix` otherwise), `helix`, `nova` or a Discord application ID.";
      tooltip = templateOption "Tooltip of the editor icon.";
      icon = stringOption "Editor icon URL.";
    };

    display = {
      theme = enumOption ["default" "atom" "catppuccin" "minecraft" "void" "classic"] "Icon theme.";
      flavor = enumOption ["dark" "light" "accent"] "Icon theme flavor.";
      view = enumOption ["full" "editor" "asset" "auto"] "What the large and small images show.";
      swapFields = boolOption "Swap the details and state lines.";
      swapIcons = boolOption "Swap the large and small images.";
    };

    timestamp = {
      enabled = boolOption "Show the elapsed time.";
      resetOnIdle = boolOption "Reset the timestamp when going idle.";
      resetOnChange = boolOption "Reset the timestamp on every activity change.";
      shared = boolOption "Share one timestamp between editor instances.";
    };

    idle = {
      enabled = boolOption "Detect idling.";
      timeout = intOption "Milliseconds without activity before idling.";
      showStatus = boolOption "Show the idle activity (otherwise clear it).";
      ignoreFocus = boolOption "Idle even while Helix is focused.";
      unidleOnFocus = boolOption "Leave idle when Helix gains focus.";
      smartIdle = boolOption "Hand the presence to the most recently active instance.";
      details = templateOption "Idle details line.";
      state = templateOption "Idle state line.";
      tooltip = templateOption "Idle icon tooltip.";
      icon = templateOption "Idle icon URL.";
    };

    text =
      {default = textOption "Text for every activity whose text is not set.";}
      // genAttrs [
        "workspace"
        "viewing"
        "editing"
        "fileBrowser"
        "pluginManager"
        "lsp"
        "docs"
        "vcs"
        "notes"
        "debug"
        "test"
        "games"
        "diagnostics"
        "terminal"
        "dashboard"
      ] (name: textOption "Text for ${name} activities: template, `false` to clear, `true` to skip the update.");

    variables =
      plainKey (leaf (types.either types.bool (types.attrsOf (types.either types.str raw))) render
        "`true` to enable `\${...}` placeholders, or extra variables (strings or `{ __raw = ...; }` procedures).");

    advanced = {
      plugin = {
        autocmds = boolOption "Update the activity on cursor movement.";
        cursorUpdate = enumOption ["on-hold" "on-move" "none"] "When cursor movement updates the activity.";
        matchInMappings = boolOption "Match file extensions in the icon mappings.";
        debounce = {
          delay = intOption "Milliseconds to let events settle before updating.";
          interval = intOption "Minimum milliseconds between updates.";
        };
      };
      server = {
        pipePath = stringOption "Path of the server pipe.";
        executablePath = stringOption "Prefix of the server executable (`<prefix>/bin/cord`); defaults to the packaged server.";
        timeout = intOption "Milliseconds the server keeps running without clients.";
      };
      discord = {
        pipePaths = stringsOption "Custom Discord IPC pipe paths.";
        reconnect = {
          enabled = boolOption "Reconnect to Discord when the connection is lost.";
          interval = intOption "Milliseconds between reconnection attempts.";
          initial = boolOption "Keep trying when the first connection fails.";
        };
        sync = {
          enabled = boolOption "Resend or rate-limit activity updates.";
          mode = enumOption ["periodic" "defer"] "`periodic` resends the last activity every interval; `defer` sends at most one update per interval.";
          interval = intOption "Milliseconds between synchronizations.";
          resetOnUpdate = boolOption "Restart the interval on activity updates.";
          pad = boolOption "Append random padding to details and state when syncing.";
        };
      };
      workspace = {
        rootMarkers = stringsOption "Files and directories marking a workspace root.";
        limitToCwd = boolOption "Stop the workspace search at the working directory.";
      };
    };
  };

  isLeaf = v: v ? _leaf;

  schemeKey = name: l:
    toKebab name
    + optionalString (isLeaf l && isBoolLeaf l && !(l.plainKey or false)) "?";

  mkOptions = tree:
    mapAttrs (name: node:
      if isLeaf node
      then
        mkOption {
          type = types.nullOr node.type;
          default = null;
          inherit (node) description;
        }
      else
        mkOption {
          type = types.submodule {options = mkOptions node;};
          default = {};
          description = "`${toKebab name}` options.";
        })
    tree;

  renderEntries = tree: values:
    concatLists (mapAttrsToList (name: node: let
      value = values.${name};
    in
      if isLeaf node
      then optional (value != null) (hashEntry (sym (schemeKey name node)) (node.render value))
      else let
        entries = renderEntries node value;
      in
        optional (entries != []) (hashEntry (sym (toKebab name)) (hash entries)))
    tree);
in {
  inherit toKebab raw;
  options = mkOptions schema;
  renderEntries = renderEntries schema;
}
