;;; dump.el --- batch org-element JSON for the parity harness -*- lexical-binding: t; -*-

;; The Org buffer is inserted, never evaluated. Babel is not executed.
;; enable-local-variables is nil before the file is read.

(setq org-element-use-cache nil
      org-element-cache-persistent nil
      enable-local-variables nil
      enable-local-eval nil
      make-backup-files nil
      create-lockfiles nil)

(require 'org)
(require 'org-element)
(require 'org-inlinetask)
(require 'json)
(require 'subr-x)

(defconst parity-default-link-types
  '("https" "http" "mailto" "file" "ftp" "news" "shell" "elisp" "help" "id"))

(defun parity-take-args ()
  "Consume command-line-args-left, including a leading \"--\"."
  (let ((args command-line-args-left))
    (setq command-line-args-left nil)
    (when (equal (car args) "--")
      (setq args (cdr args)))
    args))

(defun parity-obj (&rest kvs)
  "Hash table from a flat list of key/value pairs. :omit drops a key."
  (let ((table (make-hash-table :test 'equal)))
    (while kvs
      (let ((key (pop kvs))
            (value (pop kvs)))
        (unless (eq value :omit)
          (puthash (if (symbolp key) (symbol-name key) key) value table))))
    table))

(defun parity-pos (pos)
  "0-based byte offset for an Emacs position, or JSON null."
  (if (integerp pos)
      (- (position-bytes pos) 1)
    :null))

(defun parity-vec (items)
  (apply #'vector items))

(defun parity-read-json (path)
  (with-temp-buffer
    (insert-file-contents path)
    (json-parse-buffer :object-type 'hash-table
                       :array-type 'list
                       :null-object nil
                       :false-object :false)))

(defun parity-secondary-objects (value)
  "Object nodes from an org secondary string. Plain strings are omitted."
  (let (out)
    (when (listp value)
      (dolist (item value)
        (unless (stringp item)
          (when (and item (org-element-type item))
            (push (parity-node item) out)))))
    (nreverse out)))

(defun parity-cr-blank-line-p ()
  "Point is at bol of a line whose only character is CR.
Rust's newline token swallows CRLF, so that line is blank there.
Stock `org-element-paragraph-separate` does not treat CR as blank."
  (looking-at "\r$"))

(defun parity-paragraph-between (elem start end)
  (let ((children nil)
        (props (parity-obj)))
    (dolist (child (org-element-contents elem))
      (when (and (not (stringp child))
                 child
                 (org-element-type child)
                 (integerp (org-element-begin child))
                 (>= (org-element-begin child) start)
                 (integerp (parity-end child))
                 (<= (parity-end child) end))
        (push (parity-node child) children)))
    (let ((node (parity-obj
                 'type "paragraph"
                 'begin (parity-pos start)
                 'end (parity-pos end)
                 'properties props
                 'children (parity-vec (nreverse children)))))
      (when (= start (org-element-begin elem))
        (let ((post (org-element-property :post-affiliated elem))
              (aff (parity-affiliated elem))
              (begin (org-element-begin elem)))
          (when (and (integerp post) (integerp begin) (/= post begin))
            (puthash "post_affiliated" (parity-pos post) props))
          (unless (eq aff :omit)
            (puthash "affiliated" aff node))))
      node)))

(defun parity-paragraph-nodes (elem)
  "One paragraph, or several when a CR-only line splits it."
  (let ((begin (org-element-begin elem))
        (end (org-element-end elem))
        (split nil))
    (save-excursion
      (goto-char begin)
      (while (and (not split) (< (point) end))
        (when (parity-cr-blank-line-p) (setq split t))
        (forward-line 1)))
    (if (not split)
        (list (parity-node elem))
      (let ((nodes nil)
            (start nil))
        (save-excursion
          (goto-char begin)
          (while (< (point) end)
            (if (parity-cr-blank-line-p)
                (progn
                  (while (and (< (point) end) (parity-cr-blank-line-p))
                    (forward-line 1))
                  (when start
                    (push (parity-paragraph-between elem start (point)) nodes)
                    (setq start nil)))
              (progn
                (unless start (setq start (point)))
                (forward-line 1))))
          (when start
            (push (parity-paragraph-between elem start end) nodes)))
        (nreverse nodes)))))

(defun parity-children (elem)
  (let (out)
    (dolist (child (org-element-contents elem))
      (unless (stringp child)
        (when (and child (org-element-type child))
          (dolist (node (if (eq (org-element-type child) 'paragraph)
                            (parity-paragraph-nodes child)
                          (list (parity-node child))))
            (push node out)))))
    (parity-vec (nreverse out))))

(defun parity-end (elem)
  "Buffer position where the CST node ends.
Object :end includes :post-blank. The CST object does not."
  (let ((end (org-element-end elem)))
    (if (and (integerp end)
             (memq (org-element-type elem) org-element-all-objects))
        (let ((blank (org-element-property :post-blank elem)))
          (if (and (integerp blank) (> blank 0))
              (- end blank)
            end))
      end)))

(defun parity-affiliated (elem)
  (let ((table (make-hash-table :test 'equal))
        (begin (org-element-begin elem))
        (post (org-element-property :post-affiliated elem)))
    ;; :name on an entity is the entity name. Affiliated keywords move
    ;; :post-affiliated past :begin.
    (when (and (integerp begin) (integerp post) (> post begin))
      (parity-aff-single table elem "name" :name)
      (parity-aff-single table elem "plot" :plot)
      (parity-aff-single table elem "results" :results)
      (parity-aff-string-list table elem "header" :header)
      (parity-aff-caption table elem)
      (org-element-properties-map
       (lambda (prop _value)
         (when (keywordp prop)
           (let* ((raw (symbol-name prop))
                  (name (if (eq (aref raw 0) ?:) (substring raw 1) raw)))
             (when (string-prefix-p "attr_" name)
               (parity-aff-string-list table elem name prop)))))
       elem))
    (if (zerop (hash-table-count table))
        :omit
      table)))

(defun parity-aff-single (table elem json-key prop)
  (let ((value (org-element-property prop elem)))
    (cond
     ((stringp value) (puthash json-key value table))
     ((and (consp value) (stringp (car value)))
      (puthash json-key (car value) table)
      (when (stringp (cdr value))
        (puthash (concat json-key "_dual") (cdr value) table))))))

(defun parity-aff-string-list (table elem json-key prop)
  (let ((value (org-element-property prop elem))
        (out nil))
    (cond
     ((stringp value) (setq out (list value)))
     ((and (listp value) (cl-every #'stringp value)) (setq out value))
     ((and (listp value) (cl-every #'consp value))
      (setq out (mapcar (lambda (cell) (if (stringp (car cell)) (car cell) "")) value))))
    (when out
      (puthash json-key (parity-vec out) table))))

(defun parity-aff-caption (table elem)
  (let ((value (org-element-property :caption elem))
        (caps nil)
        (duals nil)
        (saw nil))
    (dolist (cell (if (listp value) value nil))
      (let* ((parsed (and (consp cell) (not (org-element-type cell)) (listp (car cell))))
             (main (if parsed (car cell) cell))
             (dual (and parsed (cdr cell))))
        (push (parity-vec (parity-secondary-objects main)) caps)
        (if dual
            (progn
              (setq saw t)
              (push (parity-vec (parity-secondary-objects dual)) duals))
          (push :null duals))))
    (when caps
      (puthash "caption" (parity-vec (nreverse caps)) table))
    (when saw
      (puthash "caption_dual" (parity-vec (nreverse duals)) table))))

(defun parity-title-span (elem)
  "Byte span of the headline title, matching HEADING_TITLE extent."
  (save-excursion
    (save-restriction
      (goto-char (org-element-begin elem))
      (narrow-to-region (line-beginning-position) (line-end-position))
      (goto-char (point-min))
      (when (looking-at "\\*+")
        (goto-char (match-end 0)))
      (when (looking-at "[ \t]+")
        (goto-char (match-end 0)))
      (let ((todo (org-element-property :todo-keyword elem)))
        (when (and (stringp todo) (looking-at (regexp-quote todo)))
          (goto-char (match-end 0))
          (when (looking-at "[ \t]+")
            (goto-char (match-end 0)))))
      (when (org-element-property :priority elem)
        (when (looking-at "\\[#[A-Za-z0-9]+\\]")
          (goto-char (match-end 0))
          (when (looking-at "[ \t]+")
            (goto-char (match-end 0)))))
      (when (org-element-property :commentedp elem)
        (when (looking-at "COMMENT")
          (goto-char (match-end 0))
          (when (looking-at "[ \t]+")
            (goto-char (match-end 0)))))
      (let ((start (point))
            (end (point-max)))
        (when (re-search-forward "[ \t]\\(:\\(?:[[:alnum:]_@#%]+:\\)+\\)[ \t]*\\'" nil t)
          (setq end (match-beginning 1)))
        (when (< end start)
          (setq end start))
        (cons (parity-pos start) (parity-pos end))))))

(defun parity-headline-props (elem)
  (let* ((level (org-element-property :level elem))
         (true-level (org-element-property :true-level elem))
         (span (parity-title-span elem))
         (title (parity-secondary-objects (org-element-property :title elem)))
         (tags (org-element-property :tags elem))
         (todo (org-element-property :todo-keyword elem))
         (priority (org-element-property :priority elem))
         (props (parity-obj
                 'level (or level :null)
                 'todo_keyword (if (stringp todo) todo :null)
                 'priority (if (integerp priority) priority :null)
                 'tags (parity-vec (or tags nil))
                 'commented (if (org-element-property :commentedp elem) t :false)
                 'title_begin (car span)
                 'title_end (cdr span))))
    (when (and (integerp level) (integerp true-level) (/= level true-level))
      (error "org-odd-levels-only leaked: level %s true-level %s" level true-level))
    (cons props (if title (parity-obj 'title (parity-vec title)) nil))))

(defun parity-node-json (elem type props children &optional secondary)
  (let* ((begin (org-element-begin elem))
         (end (parity-end elem))
         (post (org-element-post-affiliated elem))
         (aff (parity-affiliated elem))
         (obj (parity-obj
               'type type
               'begin (parity-pos begin)
               'end (parity-pos end)
               'properties props
               'children children)))
    (when (and (integerp post) (integerp begin) (/= post begin))
      (puthash "post_affiliated" (parity-pos post) props))
    (unless (eq aff :omit)
      (puthash "affiliated" aff obj))
    (when secondary
      (puthash "secondary" secondary obj))
    obj))

(defun parity-stamp (value)
  (if (and value (org-element-type value))
      (parity-node value)
    :null))

(defun parity-planning (elem)
  (parity-node-json
   elem "planning"
   (parity-obj
    'scheduled (parity-stamp (org-element-property :scheduled elem))
    'deadline (parity-stamp (org-element-property :deadline elem))
    'closed (parity-stamp (org-element-property :closed elem)))
   (vector)))

(defun parity-clock (elem)
  (let ((duration (org-element-property :duration elem)))
    (parity-node-json
     elem "clock"
     (parity-obj
      'value (parity-stamp (org-element-property :value elem))
      'duration (if (stringp duration) duration :null))
     (vector))))

(defun parity-table (elem)
  (let ((formulas (org-element-property :tblfm elem))
        (props (parity-obj)))
    (when formulas
      (puthash "tblfm" (parity-vec (nreverse (copy-sequence formulas))) props))
    (parity-node-json elem "table" props (parity-children elem))))

(defun parity-row (elem)
  (let ((kind (org-element-property :type elem))
        (props (parity-obj)))
    (when (or (eq kind 'rule) (equal kind "rule"))
      (puthash "rule" t props))
    (parity-node-json elem "table-row" props (parity-children elem))))

(defun parity-keyword (elem)
  (let ((key "")
        (value "")
        (post (org-element-property :post-affiliated elem)))
    (save-excursion
      ;; :begin is the affiliated run when one exists. The keyword line
      ;; starts at :post-affiliated. `\S-*` is greedy, so the colon is the
      ;; last one; the match is kept before Org upcases :key.
      (goto-char (if (integerp post) post (org-element-begin elem)))
      (beginning-of-line)
      (when (looking-at "[ \t]*#\\+\\(\\S-*\\):[ \t]*\\(.*\\)")
        (setq key (match-string-no-properties 1))
        (setq value (match-string-no-properties 2))))
    (parity-node-json elem "keyword" (parity-obj 'key key 'value value) (vector))))

(defun parity-call-prop (elem name)
  (let ((value (org-element-property name elem)))
    (if (stringp value) value :null)))

(defun parity-babel (elem type)
  (parity-node-json
   elem type
   (parity-obj
    'name (parity-call-prop elem :call)
    'inside_header (parity-call-prop elem :inside-header)
    'arguments (parity-call-prop elem :arguments)
    'end_header (parity-call-prop elem :end-header))
   (vector)))

(defun parity-link (elem)
  (let ((begin (org-element-begin elem))
        (end (parity-end elem))
        (contents-begin (org-element-contents-begin elem))
        (contents-end (org-element-contents-end elem))
        (props (parity-obj))
        (children (vector)))
    (save-excursion
      (goto-char begin)
      (cond
       ((looking-at "\\[\\[")
        (let ((path-end (if contents-begin (- contents-begin 2) (- end 2))))
          (let ((raw (buffer-substring-no-properties (+ begin 2) path-end)))
            (puthash "path" raw props)
            (puthash "unescaped" (org-link-unescape
                                  (replace-regexp-in-string "[ \t]*\n[ \t]*" " " raw))
                     props)))
        (when contents-begin
          (puthash "description_begin" (parity-pos contents-begin) props)
          (puthash "description_end" (parity-pos contents-end) props)
          (setq children (parity-children elem))))
       ((looking-at "<")
        (puthash "path" (buffer-substring-no-properties (1+ begin) (1- end)) props)
        (let ((type (org-element-property :type elem)))
          (when (stringp type)
            (puthash "type" type props))))
       (t
        (puthash "path" (buffer-substring-no-properties begin end) props)
        (let ((type (org-element-property :type elem)))
          (when (stringp type)
            (puthash "type" type props))))))
    (parity-node-json elem "link" props children)))

(defun parity-item (elem)
  (let* ((bullet (org-element-property :bullet elem))
         (box (org-element-property :checkbox elem))
         (counter (org-element-property :counter elem))
         (tag (parity-secondary-objects (org-element-property :tag elem)))
         (props (parity-obj
                 'bullet (if (stringp bullet)
                             (replace-regexp-in-string "[ \t]+\\'" "" bullet)
                           "")
                 'checkbox (pcase box
                             ('on "on")
                             ('off "off")
                             ('trans "trans")
                             (_ :null))
                 'counter (cond
                           ((integerp counter) (number-to-string counter))
                           ((stringp counter) counter)
                           (t :null))))
         (secondary (when tag (parity-obj 'tag (parity-vec tag)))))
    (parity-node-json elem "item" props (parity-children elem) secondary)))

(defun parity-citation (elem)
  (let* ((prefix (parity-secondary-objects (org-element-property :prefix elem)))
         (suffix (parity-secondary-objects (org-element-property :suffix elem)))
         (secondary (parity-obj
                     'prefix (if prefix (parity-vec prefix) :omit)
                     'suffix (if suffix (parity-vec suffix) :omit))))
    (parity-node-json
     elem "citation" (parity-obj)
     (parity-vec
      (let (refs)
        (dolist (child (org-element-contents elem))
          (when (eq (org-element-type child) 'citation-reference)
            (push (parity-node child) refs)))
        (nreverse refs)))
     (when (or prefix suffix) secondary))))

(defun parity-citation-ref (elem)
  (let* ((prefix (parity-secondary-objects (org-element-property :prefix elem)))
         (suffix (parity-secondary-objects (org-element-property :suffix elem)))
         (secondary (parity-obj
                     'prefix (if prefix (parity-vec prefix) :omit)
                     'suffix (if suffix (parity-vec suffix) :omit))))
    (parity-node-json
     elem "citation-reference" (parity-obj) (vector)
     (when (or prefix suffix) secondary))))

(defun parity-leaf (elem type)
  (let ((begin (org-element-begin elem))
        (end (parity-end elem)))
    (parity-node-json
     elem type
     (parity-obj 'value (buffer-substring-no-properties begin end))
     (vector))))

(defun parity-maybe-diary (elem node)
  "A diary sexp inside a timestamp is a child, not only a raw string."
  (when (eq (org-element-type elem) 'timestamp)
    (let ((children (gethash "children" node))
          (have nil))
      (dotimes (i (length children))
        (when (equal (gethash "type" (aref children i)) "diary-sexp")
          (setq have t)))
      (unless have
        (save-excursion
          (goto-char (org-element-begin elem))
          (let ((limit (org-element-end elem)))
            (when (search-forward "%%(" limit t)
              (let ((open (1- (point)))
                    (close nil))
                (condition-case nil
                    (progn
                      (goto-char open)
                      (forward-sexp 1)
                      (setq close (point)))
                  (scan-error nil))
                (when close
                  (let ((child (parity-obj
                                'type "diary-sexp"
                                'begin (parity-pos open)
                                'end (parity-pos close)
                                'properties (parity-obj)
                                'children (vector))))
                    (puthash "children" (vconcat children (vector child)) node)))))))))))

(defun parity-block-props (elem type)
  (let ((props (parity-obj)))
    (pcase type
      ("src-block"
       (let ((language (org-element-property :language elem)))
         (when (stringp language)
           (puthash "language" language props))))
      ("special-block"
       (let ((name (or (org-element-property :type elem)
                       (org-element-property :name elem))))
         (when (stringp name)
           (puthash "name" name props))))
      ("dynamic-block"
       (let ((name (or (org-element-property :block-name elem)
                       (org-element-property :name elem))))
         (when (stringp name)
           (puthash "name" name props))))
      ("drawer"
       (let ((name (org-element-property :drawer-name elem)))
         (when (stringp name)
           (puthash "name" name props)))))
    props))

(defun parity-footnote-label (elem)
  (let ((label (org-element-property :label elem)))
    (cond
     ((stringp label) label)
     ((null label) :null)
     (t (format "%s" label)))))

(defun parity-node (elem)
  (let* ((type (symbol-name (org-element-type elem)))
         (node
          (pcase (org-element-type elem)
            ('org-data
             (let ((root (parity-node-json elem "org-data" (parity-obj) (parity-children elem))))
               (puthash "org_version" (org-version) root)
               (puthash "emacs_version" (emacs-version) root)
               root))
            ((or 'headline 'inlinetask)
             (let ((fields (parity-headline-props elem)))
               (parity-node-json elem type (car fields) (parity-children elem) (cdr fields))))
            ('planning (parity-planning elem))
            ('clock (parity-clock elem))
            ('table (parity-table elem))
            ('table-row (parity-row elem))
            ('keyword (parity-keyword elem))
            ((or 'babel-call 'inline-babel-call) (parity-babel elem type))
            ('inline-src-block
             (parity-node-json
              elem type
              (parity-obj
               'language (parity-call-prop elem :language)
               'parameters (parity-call-prop elem :parameters)
               'body (parity-call-prop elem :value))
              (vector)))
            ('link (parity-link elem))
            ('item (parity-item elem))
            ('citation (parity-citation elem))
            ('citation-reference (parity-citation-ref elem))
            ((or 'verbatim 'code 'entity 'export-snippet 'statistics-cookie 'macro 'line-break)
             (parity-leaf elem type))
            ('node-property
             (let ((key (org-element-property :key elem))
                   (value (org-element-property :value elem)))
               (parity-node-json
                elem type
                (parity-obj
                 'key (if (stringp key) key "")
                 'value (if (stringp value) (string-trim value) :null))
                (vector))))
            ((or 'footnote-definition 'footnote-reference)
             (parity-node-json
              elem type
              (parity-obj 'label (parity-footnote-label elem))
              (parity-children elem)))
            ((or 'src-block 'special-block 'dynamic-block 'drawer)
             (parity-node-json elem type (parity-block-props elem type) (parity-children elem)))
            ('timestamp
             (let ((node (parity-node-json
                          elem type
                          (parity-obj
                           'raw (buffer-substring-no-properties
                                 (org-element-begin elem)
                                 (parity-end elem)))
                          (parity-children elem))))
               (parity-maybe-diary elem node)
               node))
            (_
             (parity-node-json elem type (parity-obj) (parity-children elem))))))
    node))

(defun parity-clamp-links (types)
  (setq org-link-parameters nil)
  (dolist (type (sort (copy-sequence types)
                      (lambda (a b) (> (length a) (length b)))))
    (org-link-set-parameters type))
  (when (fboundp 'org-link-make-regexps)
    (org-link-make-regexps)))

(defun parity-todo-keywords (todo)
  (mapcar
   (lambda (seq)
     (append (list (intern (gethash "interpretation" seq)))
             (gethash "active" seq)
             '("|")
             (gethash "done" seq)))
   todo))

(defun parity-entities (entities)
  (mapcar
   (lambda (entity)
     (list (gethash "name" entity)
           (gethash "latex" entity)
           (eq t (gethash "latex_mathp" entity))
           (gethash "html" entity)
           (gethash "ascii" entity)
           (gethash "latin1" entity)
           (gethash "utf8" entity)))
   entities))

(defun parity-apply-sidecar (side)
  (let ((level (gethash "inline_task_min_level" side))
        (alpha (gethash "list_alphabetical" side))
        (todo (gethash "todo" side))
        (links (gethash "link_types" side))
        (entities (gethash "user_entities" side)))
    (when (numberp level)
      (setq-local org-inlinetask-min-level level))
    (when (eq alpha t)
      (setq-local org-list-allow-alphabetical t))
    (when todo
      (setq-default org-todo-keywords (parity-todo-keywords todo)))
    (when entities
      (setq-local org-entities-user (parity-entities entities)))
    ;; After org-load-modules-maybe, which org-mode already ran.
    (parity-clamp-links (if links links parity-default-link-types))))

(defun parity-insert-file (path)
  ;; utf-8, not utf-8-with-signature, so U+FEFF stays in the buffer.
  ;; Clearing the auto-coding alists stops signature detection from
  ;; overriding coding-system-for-read. EOL conversion stays off so CR
  ;; remains a byte the Rust side also counts.
  (setq inhibit-eol-conversion t
        coding-system-for-read 'utf-8)
  (let ((auto-coding-alist nil)
        (auto-coding-regexp-alist nil)
        (auto-coding-functions nil))
    (insert-file-contents path)))

(defun parity-dump (file sidecar-path)
  ;; The nesting fixture walks a parent chain past the default eval depth.
  (setq max-lisp-eval-depth 10000
        max-specpdl-size 25000)
  (let ((side (if (and sidecar-path (file-readable-p sidecar-path))
                  (parity-read-json sidecar-path)
                (make-hash-table :test 'equal))))
    (with-current-buffer (get-buffer-create " *parity-parity*")
      (erase-buffer)
      (setq enable-local-variables nil
            enable-local-eval nil)
      (parity-insert-file file)
      (setq org-element-use-cache nil
            org-element-cache-persistent nil)
      (org-mode)
      (setq-local org-element-use-cache nil)
      (setq-local org-element-cache-persistent nil)
      (setq-local org-odd-levels-only nil)
      (parity-apply-sidecar side)
      (org-set-regexps-and-options)
      (setq-local org-odd-levels-only nil)
      (let ((tree (org-element-parse-buffer 'object)))
        (princ (json-serialize (parity-node tree)))
        (princ "\n")))))

(defun parity-priority-triple (line)
  (with-temp-buffer
    (org-mode)
    (insert "#+PRIORITIES: " line "\n")
    (org-set-regexps-and-options)
    (vector org-priority-highest org-priority-lowest org-priority-default)))

(defun parity-priorities (path)
  (let ((table (parity-read-json path))
        (rows nil))
    (dolist (row table)
      (push (parity-priority-triple (gethash "line" row)) rows))
    (princ (json-serialize
            (parity-obj
             'org_version (org-version)
             'rows (parity-vec (nreverse rows)))))
    (princ "\n")))

(defun parity-syntax-probe ()
  (with-temp-buffer
    (org-mode)
    (let ((byte 0))
      (while (<= byte 127)
        (princ (format "%d %c\n" byte (char-syntax byte)))
        (setq byte (1+ byte))))))

(defun parity-main ()
  (let ((args (parity-take-args)))
    (pcase (car args)
      ("--syntax-probe" (parity-syntax-probe))
      ("--priorities" (parity-priorities (cadr args)))
      (_
       (unless (and (car args) (cadr args))
         (error "Usage: dump.el -- FILE SIDECAR"))
       (parity-dump (car args) (cadr args))))))

(parity-main)
