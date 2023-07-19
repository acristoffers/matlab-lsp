(command_name) @function
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
  "case"
  "catch"
  "classdef"
  "else"
  "end"
  "enumeration"
  "events"
  "for"
  "function"
  "global"
  "if"
  "methods"
  "otherwise"
  "parfor"
  "persistent"
  "properties"
  "switch"
  "try"
  "while"
] @keyword
