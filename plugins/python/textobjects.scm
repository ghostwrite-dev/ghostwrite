; Text objects for python: what `maf`, `]f` and the like select. From helix
; (https://github.com/helix-editor/helix, runtime/queries), under the Mozilla
; Public License 2.0, with the files it inherits from put in this one.

(function_definition
  body: (block)? @function.inside) @function.around

(class_definition
  body: (block)? @class.inside) @class.around

(parameters
  ((_) @parameter.inside . ","? @parameter.around) @parameter.around)
  
(lambda_parameters
  ((_) @parameter.inside . ","? @parameter.around) @parameter.around)

(argument_list
  ((_) @parameter.inside . ","? @parameter.around) @parameter.around)

(comment) @comment.inside

(comment)+ @comment.around

((function_definition
   name: (identifier) @_name
   body: (block)? @test.inside) @test.around
 (#match? @_name "^test_"))

(list
  (_) @entry.around)

(tuple
  (_) @entry.around)

(set
  (_) @entry.around)

(pair
  (_) @entry.inside) @entry.around
