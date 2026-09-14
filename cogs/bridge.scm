;;; Forwards editor events to `libcord_hx` and performs the actions it sends back.

(require "helix/editor.scm")
(require "helix/misc.scm")
(require (prefix-in helix. "helix/commands.scm"))
(require (prefix-in helix.static. "helix/static.scm"))

(#%require-dylib "libcord_hx"
                 (prefix-in native.
                            (only-in cord-attach
                                     cord-next-action
                                     cord-reply
                                     cord-start
                                     cord-configure
                                     cord-event
                                     cord-command
                                     cord-manager
                                     cord-icon
                                     cord-run)))

(provide bridge-start!
         bridge-configure!
         bridge-command!
         bridge-manager!
         bridge-icon
         bridge-run
         bridge-schedule)

;; Callbacks

;; Helix keeps the procedures handed to it (callbacks, awaited futures, hooks)
;; alive when the Steel engine is reloaded (workspace trust,
;; `:config-reload`). If such a procedure still references the old engine's
;; heap, e.g. through mutated captured variables or continuations, the next
;; garbage collection of the new engine panics and takes Helix down.
;; Procedures are therefore kept in this table, and Helix only receives
;; trampolines capturing their numeric id.
(define *callbacks* (hash))
(define *next-callback* 0)

(define (callback-register! f)
  (set! *next-callback* (+ *next-callback* 1))
  (set! *callbacks* (hash-insert *callbacks* *next-callback* f))
  *next-callback*)

(define (callback-take! id)
  (let ([f (hash-try-get *callbacks* id)])
    (set! *callbacks* (hash-remove *callbacks* id))
    f))

;; Called at most once.
(define (oneshot f)
  (let ([id (callback-register! f)])
    (lambda args
      (let ([f (callback-take! id)])
        (when f
          (apply f args))))))

(define (persistent f)
  (let ([id (callback-register! f)])
    (lambda args
      (apply (hash-ref *callbacks* id) args))))

(define (bridge-schedule thunk)
  (enqueue-thread-local-callback (oneshot thunk)))

;; Values

;; Procedures handed to the core, by id.
(define *procs* (hash))
(define *next-proc* 0)

;; Converts a value for the core: symbols become strings and procedures
;; become `(hash "__cord_proc" id)` markers.
(define (->native v)
  (cond
    [(symbol? v) (symbol->string v)]
    [(procedure? v)
     (set! *next-proc* (+ *next-proc* 1))
     (set! *procs* (hash-insert *procs* *next-proc* v))
     (hash "__cord_proc" *next-proc*)]
    [(hash? v)
     (let loop ([keys (hash-keys->list v)] [out (hash)])
       (if (null? keys)
           out
           (let ([k (car keys)])
             (loop (cdr keys)
                   (hash-insert out (if (symbol? k) (symbol->string k) k) (->native (hash-ref v k)))))))]
    [(list? v) (map ->native v)]
    [(vector? v) (map ->native (vector->list v))]
    [(char? v) (string v)]
    [else v]))

;; Converts a value from the core: hash map keys become symbols and markers
;; become the procedures they stand for.
(define (->scheme v)
  (cond
    [(hash? v)
     (if (hash-contains? v "__cord_proc")
         (hash-ref *procs* (hash-ref v "__cord_proc"))
         (let loop ([keys (hash-keys->list v)] [out (hash)])
           (if (null? keys)
               out
               (let ([k (car keys)])
                 (loop (cdr keys) (hash-insert out (string->symbol k) (->scheme (hash-ref v k))))))))]
    [(vector? v) (map ->scheme (vector->list v))]
    [(list? v) (map ->scheme v)]
    [else v]))

;; Custom extensions may be given as `(hash 'setup f)`: `f` takes the
;; extension's configuration and returns the extension.
(define (setup-extension entry)
  (cond
    [(and (hash? entry) (procedure? (hash-try-get entry 'setup))) ((hash-ref entry 'setup) (hash))]
    [(and (list? entry) (= (length entry) 2) (hash? (car entry)) (procedure? (hash-try-get (car entry) 'setup)))
     ((hash-ref (car entry) 'setup) (cadr entry))]
    [else entry]))

(define (prepare config)
  (let ([config (if (hash? config) config (hash))])
    (->native (if (list? (hash-try-get config 'extensions))
                  (hash-insert config 'extensions (map setup-extension (hash-ref config 'extensions)))
                  config))))

;; Editor

;; Whether the focused view still exists. Helix panics when view accessors are
;; used after the last view was closed, which happens to hooks and callbacks
;; running while the editor quits. After the last view closes, the focus
;; points at the empty root container; a document being shown in any view
;; means the focus is one of them.
(define (focused-view-alive?)
  (let loop ([docs (editor-all-documents)])
    (cond
      [(null? docs) #f]
      [(editor-doc-in-view? (car docs)) #t]
      [else (loop (cdr docs))])))

(define (current-doc)
  (and (focused-view-alive?) (editor->doc-id (editor-focus))))

;; `editor-document-diagnostic-counts` of this Helix build, or #f.
(define *diagnostic-counts* 'unknown)

(define (diagnostic-counts)
  (when (eq? *diagnostic-counts* 'unknown)
    (set! *diagnostic-counts*
          (with-handler (lambda (err) #f)
                        (eval '(require-builtin helix/core/editor as cord-diagnostics.))
                        (eval 'cord-diagnostics.editor-document-diagnostic-counts))))
  *diagnostic-counts*)

(define (counts-of f doc)
  (let ([c (with-handler (lambda (err) #f) (f doc))])
    (if (vector? c) (vector->list c) c)))

(define (sum-counts a b)
  (if (and (list? a) (list? b)) (map + a b) (or a b)))

(define (snapshot)
  (let* ([doc (current-doc)]
         [doc (and doc (editor-doc-exists? doc) doc)]
         [path (and doc (editor-document->path doc))]
         [language (and doc (editor-document->language doc))]
         [counts (diagnostic-counts)])
    (hash "path" (if (string? path) path "")
          "language" (if (string? language) language "")
          "line" (if doc (+ (helix.static.get-current-line-number) 1) 1)
          "column" (if doc (+ (helix.static.get-current-column-number) 1) 1)
          "cwd" (helix.static.get-helix-cwd)
          "diagnostics" (and counts doc (counts-of counts doc))
          "workspace-diagnostics"
          (and counts
               (let loop ([docs (editor-all-documents)] [total #f])
                 (if (null? docs) total (loop (cdr docs) (sum-counts total (counts-of counts (car docs))))))))))

(define *last-doc* #f)

(define (event! kind)
  (native.cord-event kind (snapshot)))

(define (check-buffer-change)
  (let ([doc (current-doc)])
    ;; no focused view: the editor is closing, not switching buffers
    (unless (or (not doc) (equal? doc *last-doc*))
      (set! *last-doc* doc)
      (event! "buf-enter"))))

(define (on-cursor-moved)
  (check-buffer-change)
  (event! "cursor"))

(define (hook! event callback)
  (with-handler (lambda (err) (log::warn! (string-append "[cord] hook '" (symbol->string event) "' is unavailable")))
                (register-hook event (persistent callback))))

(define (register-hooks!)
  (set! *last-doc* (current-doc))
  (hook! 'selection-did-change (lambda (view-id) (on-cursor-moved)))
  (hook! 'document-changed (lambda (doc-id old-text) (on-cursor-moved)))
  (hook! 'document-opened (lambda (doc-id) (check-buffer-change)))
  (hook! 'document-closed (lambda (event) (bridge-schedule check-buffer-change)))
  (hook! 'post-command (lambda (command) (check-buffer-change)))
  (hook! 'terminal-focus-gained (lambda () (event! "focus-gained")))
  (hook! 'terminal-focus-lost (lambda () (event! "focus-lost"))))

;; Actions

(define (notify message level)
  (cond
    [(>= level 4) (set-error! message)]
    [(= level 3) (set-warning! message)]
    [else (set-status! message)]))

(define (call! action)
  (let* ([proc (hash-try-get *procs* (hash-ref action "proc"))]
         [result (if proc
                     (with-handler (lambda (err) (hash "__cord_error" (to-string err)))
                                   (->native (apply proc (->scheme (hash-ref action "args")))))
                     (hash "__cord_error" "unknown procedure"))])
    (native.cord-reply (hash-ref action "session") (hash-ref action "id") result)))

(define (perform! action)
  (let ([type (hash-ref action "type")])
    (cond
      [(equal? type "call") (call! action)]
      [(equal? type "notify") (notify (hash-ref action "message") (hash-ref action "level"))]
      [(equal? type "scratch")
       (helix.new)
       (helix.static.insert_string (hash-ref action "content"))
       (set-scratch-buffer-name! (hash-ref action "name"))
       (helix.set-language (hash-ref action "language"))]
      [(equal? type "clipboard") (set-register! #\+ (list (hash-ref action "text")))])))

(define (next-action!)
  (helix-await-callback (native.cord-next-action)
                        (oneshot (lambda (action)
                                   (when (hash? action)
                                     (with-handler (lambda (err) (log::error! (to-string err)))
                                                   (perform! action))
                                     (next-action!))))))

;; API

(define (bridge-start! config)
  (native.cord-attach)
  (next-action!)
  (register-hooks!)
  (native.cord-start (prepare config) (snapshot)))

(define (bridge-configure! config)
  (native.cord-configure (prepare config)))

(define (bridge-command! args)
  (native.cord-command (map (lambda (a) (if (symbol? a) (symbol->string a) a)) args) (snapshot)))

(define (bridge-manager! method args)
  (native.cord-manager method (->native args)))

(define (bridge-icon name theme flavor)
  (native.cord-icon name (->native theme) (->native flavor)))

(define (bridge-run command cwd ttl)
  (native.cord-run (->native command) (->native cwd) (->native ttl)))
