; Text objects for css: what `maf`, `]f` and the like select. From helix
; (https://github.com/helix-editor/helix, runtime/queries), under the Mozilla
; Public License 2.0, with the files it inherits from put in this one.

; Rule sets and their declaration blocks
(rule_set) @function.around
(rule_set
  (block) @function.inside)

(keyframe_block_list) @function.around
(keyframe_block
  (block) @function.inside)

; At-rules with a body (e.g. @media, @supports)
(media_statement) @class.around
(media_statement
  (block) @class.inside)

; Declarations as key/value entries
(declaration) @entry.around
(declaration
  ":"
  (_) @entry.inside)

(comment) @comment.inside
(comment)+ @comment.around
