{
  perSystem = {naersk, ...}: {
    checks.tests = naersk.buildPackage {
      pname = "cord-hx-tests";
      src = ../.;
      mode = "test";
      cargoTestOptions = options: options ++ ["--lib"];
    };
  };
}
