;; Example keybindings for cord.hx, to copy into your init.scm.

(require "helix/keymaps.scm")
(require "cord/cord.scm")

(keymap (global)
  (normal
    (space
      (D
        (p ":cord-presence-toggle")
        (i ":cord-idle-toggle")
        (s ":cord-status")
        (h ":cord-health")))))
