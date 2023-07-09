(function_definition) @fndef
(lambda) @fndef

(assignment left: (function_call (identifier) @varref))
(assignment left: (identifier) @varref)
(assignment right: (identifier) @varref)
(multioutput_variable (function_call (identifier) @varref))

(assignment left: (function_call (identifier) @vardef))
(assignment left: (identifier) @vardef)
(catch_clause (identifier) @vardef)
(function_arguments (identifier) @vardef)
(function_output (identifier) @vardef)
(global_operator (identifier) @vardef)
(iterator . (identifier) @vardef)
(lambda (arguments (identifier) @vardef))
(multioutput_variable (function_call (identifier) @vardef))
(multioutput_variable (identifier) @vardef)

(field_expression) @field

(function_call) @fncall
(command_name) @command
(identifier) @identifier
