(command_name) @macro
(unary_operator (number)) @number
(number) @number
(comment) @comment
(string) @string
(identifier) @identifer
(function_arguments (identifier) @parameter)
(function_output (identifier) @parameter)
(function_output (multioutput_variable (identifier) @parameter))
(lambda (arguments (identifier) @parameter))
(handle_operator (identifier) @function)

[
  "+"
  ".+"
  "-"
  ".*"
  "*"
  ".*"
  "/"
  "./"
  "\\"
  ".\\"
  "^"
  ".^"
  "'"
  ".'"
  "|"
  "&"
  "?"
  "@"
  "<"
  "<="
  ">"
  ">="
  "=="
  "~="
  "="
  "&&"
  "||"
  ":"
] @operator

[
  "arguments"
  "classdef"
  "end"
  "enumeration"
  "events"
  "global"
  "methods"
  "persistent"
  "properties"
  "function"
] @keyword
