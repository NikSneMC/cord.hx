{
  perSystem = {
    pkgs,
    naersk,
    toolchain,
    self',
    ...
  }: let
    inherit (pkgs) removeReferencesTo runCommand;
    inherit (pkgs.lib.fileset) toSource unions;
    inherit (pkgs.stdenv.hostPlatform.extensions) sharedLibrary;

    native = naersk.buildPackage {
      pname = "cord-hx";
      src = ../.;
      copyLibs = true;

      nativeBuildInputs = [removeReferencesTo];
      postInstall = ''
        find $out -type f \( -path '*/bin/*' -o -name '*${sharedLibrary}' \) \
          -exec remove-references-to -t ${toolchain} {} +
      '';
      overrideMain = _: {disallowedReferences = [toolchain];};
    };

    nativeLib = runCommand "cord-hx-native" {} ''
      mkdir -p $out
      cp ${native}/lib/libcord_hx${sharedLibrary} $out/
    '';

    scmFiles = toSource {
      root = ../.;
      fileset = unions [
        ../cord.scm
        ../api.scm
        ../cog.scm
        ../cogs
      ];
    };
  in {
    packages = {
      inherit native;

      default = runCommand "helix-plugin-cord-${native.version}" {
        passthru = {
          cogName = "cord";
          pluginDependencies = [];
          native = nativeLib;
          server = native;
        };
      } "cp -r ${scmFiles} $out";

      cord = self'.packages.default;
    };
  };
}
