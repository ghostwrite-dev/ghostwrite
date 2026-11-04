; Text objects for bash: what `maf`, `]f` and the like select. From helix
; (https://github.com/helix-editor/helix, runtime/queries), under the Mozilla
; Public License 2.0, with the files it inherits from put in this one.

(function_definition
  body: (_) @function.inside) @function.around

(command
  argument: (_) @parameter.inside)

(comment) @comment.inside

(comment)+ @comment.around

(array
  (_) @entry.around)
