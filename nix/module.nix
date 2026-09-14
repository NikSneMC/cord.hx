{
  inputs,
  moduleWithSystem,
  ...
}: {
  flake.homeManagerModules = rec {
    cord = moduleWithSystem ({self'}: {
      config,
      lib,
      ...
    }: let
      inherit (builtins) isBool isString removeAttrs;
      inherit (lib) types mkOption mkEnableOption mkDefault optionalAttrs mkIf mapAttrsToList filterAttrs genAttrs literalExpression optional optionalString recursiveUpdate;

      scheme = import ./scheme.nix {
        inherit lib;
        inherit (inputs) nhx;
      };
      schema = import ./options.nix {inherit lib scheme;};
      inherit (schema) toKebab raw renderEntries options;
      inherit (scheme) call hash hashEntry list render renderWith str sym;

      plugin = config.programs.nhx.plugins.cord;
      cfg = plugin.config;

      textType = types.either types.str raw;

      oneOrMany = types.either types.str (types.listOf types.str);

      rule = types.submodule {
        options = {
          path = mkOption {
            type = types.nullOr oneOrMany;
            default = null;
            description = "Workspace directory, or list of directories, matched exactly. `~` is the home directory.";
            example = "~/Projects/secret";
          };
          name = mkOption {
            type = types.nullOr oneOrMany;
            default = null;
            description = "Workspace (repository) name, or list of names.";
          };
          glob = mkOption {
            type = types.nullOr oneOrMany;
            default = null;
            description = "Regex, or list of regexes, matched against the buffer path (the workspace directory for buffers without a file). A leading `~` (after an optional `^`) is the home directory.";
            example = "^~/Projects/[^/]+/work";
          };
          predicate = mkOption {
            type = types.nullOr raw;
            default = null;
            description = "Scheme procedure receiving `(hash 'workspace 'workspace-dir 'filename 'path)`.";
          };
          visible = mkOption {
            type = types.nullOr types.bool;
            default = null;
            description = "`false` hides the presence while the rule matches.";
          };
          details = mkOption {
            type = types.nullOr (types.oneOf [types.str types.bool raw]);
            default = null;
            description = "Details line template (`\${workspace}`, `\${filename}`, `\${name}`, `\${cursor-line}`, ... placeholders), `false` to omit the line, `null` to keep the default.";
          };
          state = mkOption {
            type = types.nullOr (types.oneOf [types.str types.bool raw]);
            default = null;
            description = "State line template, same as `details`.";
          };
        };
      };

      button = types.submodule {
        options = {
          label = mkOption {
            type = textType;
            description = "Button label.";
          };
          url = mkOption {
            type = textType;
            description = "Button URL.";
          };
        };
      };

      asset = types.submodule {
        options = genAttrs ["name" "icon" "tooltip" "text" "type"] (name:
          mkOption {
            type = types.nullOr textType;
            default = null;
            description = "Asset ${name}.";
          });
      };

      hook = types.either raw (types.submodule {
        options = {
          callback = mkOption {
            type = raw;
            description = "Scheme procedure.";
          };
          priority = mkOption {
            type = types.nullOr types.int;
            default = null;
            description = "Higher runs first. Hooks default to 200; extensions use 0 to 100.";
          };
        };
      });

      pluginOptions =
        options
        // {
          buttons = mkOption {
            type = types.listOf button;
            default = [];
            description = "Up to two buttons.";
          };

          assets = mkOption {
            type = types.attrsOf (types.either types.str asset);
            default = {};
            example = literalExpression ''{ "Cargo.toml" = "https://example.com/cargo.png"; }'';
            description = "Icons per filetype, file name or extension: an icon URL or asset options.";
          };

          hooks = mkOption {
            type = types.attrsOf hook;
            default = {};
            example = literalExpression ''{ postActivity = { __raw = "(lambda (opts activity) activity)"; }; }'';
            description = "Hooks per event (`ready`, `preActivity`, `postActivity`, `idleEnter`, ...): Scheme procedures.";
          };

          workspaceRules = mkOption {
            type = types.listOf rule;
            default = [];
            example = literalExpression ''
              [
                { glob = "^~/Projects/[^/]+/work"; details = "Probably some work-related stuff"; state = false; }
                { path = [ "~/Projects/secret" "~/Documents" ]; visible = false; }
                { name = "cord.hx"; details = "''${workspace}"; }
              ]
            '';
            description = "Per-workspace activity overrides; the first matching rule wins. Rules match by `path`, `name`, `glob` and `predicate` (all given ones have to match).";
          };

          extensions = mkOption {
            type = types.attrsOf (types.submodule ({name, ...}: {
              freeformType = types.attrsOf types.anything;
              options.enable = mkEnableOption "the ${name} extension";
            }));
            default = {};
            example = literalExpression ''
              {
                diagnostics.enable = true;
                scopedTimestamps = { enable = true; pause = false; };
              }
            '';
            description = ''
              Built-in extensions. Besides `enable`, the attributes are the options of the
              extension, in camelCase like the other options (`localTime.affectIdle = false`).
            '';
          };
        };

      renderFreeform =
        renderWith (k: v:
          sym "${toKebab k}${optionalString (isBool v) "?"}");

      compact = filterAttrs (_: v: v != null);

      enabledExtensions = filterAttrs (_: e: e.enable) cfg.extensions;

      entry = name: value: hashEntry (sym name) value;

      extraEntries =
        optional (cfg.buttons != []) (entry "buttons" (render cfg.buttons))
        ++ optional (cfg.assets != {}) (entry "assets" (hash (mapAttrsToList (
            k: v:
              hashEntry (str k) (render (
                if isString v
                then v
                else compact v
              ))
          )
          cfg.assets)))
        ++ optional (cfg.hooks != {}) (entry "hooks" (hash (mapAttrsToList (
            event: h: entry (toKebab event) (render (compact h))
          )
          cfg.hooks)))
        ++ optional (cfg.workspaceRules != []) (entry "workspace-rules" (render (map (
            r:
              compact (removeAttrs r ["visible"])
              // optionalAttrs (r.visible != null) {"visible?" = r.visible;}
          )
          cfg.workspaceRules)))
        ++ optional (enabledExtensions != {}) (entry "extensions" (list (mapAttrsToList (
            name: value: let
              options = removeAttrs value ["enable"];
            in
              if options == {}
              then sym (toKebab name)
              else list [(sym (toKebab name)) (renderFreeform options)]
          )
          enabledExtensions)));

      entries =
        renderEntries (recursiveUpdate cfg {
          advanced.server.executablePath =
            if cfg.advanced.server.executablePath != null
            then cfg.advanced.server.executablePath
            else "${plugin.package.server}";
        })
        ++ extraEntries;
    in {
      programs.nhx.plugins.cord = {
        package = mkDefault self'.packages.default;
        config = {...}: {options = pluginOptions;};
        # `cord/api.scm` is available to procedures written as `__raw`
        rendered = mkIf plugin.enable ''
          (require "cord/api.scm")
          ${call "cord-configure!" [(hash entries)]}
        '';
      };
    });

    default = cord;
  };
}
