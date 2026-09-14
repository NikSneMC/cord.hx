{
  lib,
  nhx,
}: let
  inherit (builtins) toJSON match isAttrs isList isBool isString isInt isFloat typeOf;
  inherit (lib) concatLists mapAttrsToList optional;

  nhxScheme = import "${nhx}/lib/scheme.nix" {inherit lib;};
  inherit (nhxScheme) sym hash hashEntry list bool;
in
  nhxScheme
  // rec {
    str = toJSON;

    raw = value: {__raw = value;};

    key = k: _:
      if match "[a-zA-Z][a-zA-Z0-9?!*<>=/+_-]*" k != null
      then sym k
      else str k;

    renderWith = keyFn: value:
      if isAttrs value && value ? __raw
      then value.__raw
      else if isAttrs value
      then
        hash (concatLists (mapAttrsToList (k: v:
          optional (v != null) (hashEntry (keyFn k v) (renderWith keyFn v)))
        value))
      else if isList value
      then list (map (renderWith keyFn) value)
      else if isBool value
      then bool value
      else if isString value
      then str value
      else if isInt value || isFloat value
      then toString value
      else throw "cord-hx: cannot render ${typeOf value} as Scheme";

    render = renderWith key;
  }
