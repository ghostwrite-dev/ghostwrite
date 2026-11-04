; Text objects for cpp: what `maf`, `]f` and the like select. From helix
; (https://github.com/helix-editor/helix, runtime/queries), under the Mozilla
; Public License 2.0, with the files it inherits from put in this one.

(function_definition
  body: (_) @function.inside) @function.around

(struct_specifier
  body: (_) @class.inside) @class.around

(enum_specifier
  body: (_) @class.inside) @class.around

(union_specifier
  body: (_) @class.inside) @class.around

(parameter_list 
  ((_) @parameter.inside . ","? @parameter.around) @parameter.around)

(argument_list
  ((_) @parameter.inside . ","? @parameter.around) @parameter.around)

(comment) @comment.inside

(comment)+ @comment.around

(enumerator
  (_) @entry.inside) @entry.around

(initializer_list
  (_) @entry.around)

(lambda_expression
  body: (_) @function.inside) @function.around

(class_specifier
  body: (_) @class.inside) @class.around
