;;; cord.hx API for hooks and procedures in the configuration.
;;;
;;; ```scheme
;;; (require "cord/api.scm")
;;; (cord-configure!
;;;  (hash 'hooks (hash 'workspace-change
;;;                     (lambda (opts)
;;;                       (if (equal? (hash-ref opts 'workspace) "notes")
;;;                           (cord-hide!)
;;;                           (cord-resume!))))))
;;; ```
;;;
;;; The manager functions mirror cord.nvim's `ActivityManager`. Called from a
;;; procedure, they run once it returns, before cord continues, so
;;; `(cord-skip-update!)` in `'post-activity` skips the activity being built.

(require "cogs/bridge.scm")

(provide cord-queue-update!
         cord-skip-update!
         cord-set-activity!
         cord-clear-activity!
         cord-pause!
         cord-resume!
         cord-pause-events!
         cord-resume-events!
         cord-hide!
         cord-suppress!
         cord-toggle!
         cord-toggle-suppress!
         cord-idle!
         cord-force-idle!
         cord-unidle!
         cord-toggle-idle!
         cord-icon
         cord-run)

;;@doc
;; Queue an activity update; `(cord-queue-update! #t)` updates even when
;; nothing changed.
(define (cord-queue-update! . force) (bridge-manager! "queue-update" force))

;;@doc
;; Skip the next activity update.
(define (cord-skip-update!) (bridge-manager! "skip-update" '()))

;;@doc
;; Send an activity hash map (`'details`, `'state`, `'assets`, `'timestamps`,
;; `'buttons`, `'type`, ...). `'post-activity` hooks run on it. Pass #t to
;; ignore a pending `cord-skip-update!`.
(define (cord-set-activity! activity . force)
  (bridge-manager! "set-activity" (cons activity force)))

;;@doc
;; Clear the activity, letting other editor instances show theirs; pass #t to
;; clear the presence completely.
(define (cord-clear-activity! . force) (bridge-manager! "clear-activity" force))

;;@doc
;; Pause events and the idle timer.
(define (cord-pause!) (bridge-manager! "pause" '()))

;;@doc
;; Resume events and the idle timer.
(define (cord-resume!) (bridge-manager! "resume" '()))

;;@doc
;; Ignore editor events without stopping the idle timer.
(define (cord-pause-events!) (bridge-manager! "pause-events" '()))

;;@doc
;; Handle editor events again and update the activity.
(define (cord-resume-events!) (bridge-manager! "resume-events" '()))

;;@doc
;; Pause and clear the presence.
(define (cord-hide!) (bridge-manager! "hide" '()))

;;@doc
;; Pause and let other editor instances show their activity.
(define (cord-suppress!) (bridge-manager! "suppress" '()))

;;@doc
;; Toggle between hidden and shown.
(define (cord-toggle!) (bridge-manager! "toggle" '()))

;;@doc
;; Toggle between suppressed and shown.
(define (cord-toggle-suppress!) (bridge-manager! "toggle-suppress" '()))

;;@doc
;; Show the idle activity.
(define (cord-idle!) (bridge-manager! "idle" '()))

;;@doc
;; Stay idle until `cord-unidle!`.
(define (cord-force-idle!) (bridge-manager! "force-idle" '()))

;;@doc
;; Leave the idle state.
(define (cord-unidle!) (bridge-manager! "unidle" '()))

;;@doc
;; Toggle the idle state; `(cord-toggle-idle! #t)` forces idling.
(define (cord-toggle-idle! . force) (bridge-manager! "toggle-idle" force))

;;@doc
;; URL of a built-in icon, e.g. `(cord-icon "keyboard")` or
;; `(cord-icon "idle" "atom" "light")`. Theme and flavor default to the
;; configured ones.
(define (cord-icon name . theme-flavor)
  (bridge-icon name
               (if (null? theme-flavor) void (car theme-flavor))
               (if (or (null? theme-flavor) (null? (cdr theme-flavor))) void (cadr theme-flavor))))

;;@doc
;; Last trimmed stdout of a command (#f while none has finished or the last run
;; failed), rerun in the background once older than `ttl` seconds (default 30):
;; `(cord-run '("git" "branch" "--show-current") (hash-ref opts 'workspace-dir) 60)`
(define (cord-run command . options)
  (bridge-run command
              (if (null? options) void (car options))
              (if (or (null? options) (null? (cdr options))) 30 (cadr options))))
