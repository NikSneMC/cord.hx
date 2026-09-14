;;; cord.hx - Discord Rich Presence for Helix.
;;;
;;; ```scheme
;;; (require "cord/cord.scm")
;;; (cord-configure! (hash 'display (hash 'theme 'catppuccin)))
;;; ```

(require "cogs/bridge.scm")

(provide cord-configure!
         cord
         cord-presence-show
         cord-presence-hide
         cord-presence-suppress
         cord-presence-toggle
         cord-presence-toggle-suppress
         cord-idle
         cord-unidle
         cord-idle-toggle
         cord-idle-force
         cord-enable
         cord-disable
         cord-toggle
         cord-status
         cord-version
         cord-restart
         cord-reconnect
         cord-shutdown
         cord-health
         cord-debug
         cord-debug-copy)

(define *config* void)
(define *started?* #f)

;;@doc
;; Configure cord with a hash map of options, e.g.
;; `(cord-configure! (hash 'idle (hash 'timeout 600000)))`.
;; See the README for all options.
(define (cord-configure! config)
  (set! *config* config)
  (when *started?*
    (bridge-configure! config)))

(define (run . args)
  (bridge-command! args)
  void)

;;@doc
;; Run a cord command: presence [show|hide|suppress|toggle|toggle-suppress],
;; idle [show|hide|toggle|force], enable|disable|toggle [idle], status,
;; version, restart, reconnect, shutdown, health, debug [copy]
(define (cord . args) (apply run args))

;;@doc
;; Show the rich presence again after hiding or suppressing it
(define (cord-presence-show . _) (run "presence" "show"))

;;@doc
;; Pause cord and clear the rich presence
(define (cord-presence-hide . _) (run "presence" "hide"))

;;@doc
;; Pause cord and let another editor instance take over the rich presence
(define (cord-presence-suppress . _) (run "presence" "suppress"))

;;@doc
;; Toggle between showing and hiding the rich presence
(define (cord-presence-toggle . _) (run "presence" "toggle"))

;;@doc
;; Toggle between showing and suppressing the rich presence
(define (cord-presence-toggle-suppress . _) (run "presence" "toggle-suppress"))

;;@doc
;; Show the idle activity
(define (cord-idle . _) (run "idle" "show"))

;;@doc
;; Leave the idle state
(define (cord-unidle . _) (run "idle" "hide"))

;;@doc
;; Toggle the idle state
(define (cord-idle-toggle . _) (run "idle" "toggle"))

;;@doc
;; Enter the idle state and stay there until `:cord-unidle`
(define (cord-idle-force . _) (run "idle" "force"))

;;@doc
;; Show the rich presence, or enable a feature: `:cord-enable idle`
(define (cord-enable . feature) (apply run "enable" feature))

;;@doc
;; Hide the rich presence, or disable a feature: `:cord-disable idle`
(define (cord-disable . feature) (apply run "disable" feature))

;;@doc
;; Toggle the rich presence, or a feature: `:cord-toggle idle`
(define (cord-toggle . feature) (apply run "toggle" feature))

;;@doc
;; Show the connection status
(define (cord-status . _) (run "status"))

;;@doc
;; Show the server version
(define (cord-version . _) (run "version"))

;;@doc
;; Restart the server (affects every connected editor)
(define (cord-restart . _) (run "restart"))

;;@doc
;; Reconnect the server to Discord
(define (cord-reconnect . _) (run "reconnect"))

;;@doc
;; Stop the server
(define (cord-shutdown . _) (run "shutdown"))

;;@doc
;; Open a health report: system, server and configuration issues
(define (cord-health . _) (run "health"))

;;@doc
;; Open a debug report to attach to bug reports
(define (cord-debug . _) (run "debug"))

;;@doc
;; Copy the debug report to the clipboard
(define (cord-debug-copy . _) (run "debug" "copy"))

;; Start on the next event loop iteration so that `cord-configure!` calls
;; placed after the `require` take effect first.
(bridge-schedule (lambda ()
                   (unless *started?*
                     (set! *started?* #t)
                     (bridge-start! *config*))))
