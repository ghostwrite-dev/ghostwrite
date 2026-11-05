; Text objects for json: what `maf`, `]f` and the like select. From helix
; (https://github.com/helix-editor/helix, runtime/queries), under the Mozilla
; Public License 2.0, with the files it inherits from put in this one.

(pair 
  (_) @entry.inside) @entry.around

(array
  (_) @entry.around)

(comment) @comment.inside
(comment)+ @comment.around
