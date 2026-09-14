(define package-name 'cord)
(define version "1.0.0")
(define dependencies '())

;; Builds `libcord_hx` and, as part of the same cargo build, the `cord`
;; server executable (found under `$STEEL_HOME/target/cord/release`).
;; forge reads this file as data: keep the `'` quote shorthand.
(define dylibs '((#:name "cord-hx")))
