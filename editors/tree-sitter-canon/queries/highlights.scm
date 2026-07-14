; ─── Operators ────────────────────────────────────────────────────────────────
; `=` binds a name, `=>` declares (constructors, lambdas, dispatch arms),
; `->` executes (the pipe). `*` is both the product separator and the
; dispatch-arm bullet; `^` is type repetition (Byte^8, Byte^*).
"=" @operator
"=>" @operator
"->" @operator
"+" @operator
"*" @operator
"?" @operator
"^" @operator

; ─── Punctuation ──────────────────────────────────────────────────────────────
[
  "("
  ")"
  "{"
  "}"
  "["
  "]"
  "<"
  ">"
  "</"
  "/>"
] @punctuation.bracket

[
  "."
  ","
  ":"
] @punctuation.delimiter

; ─── Literals ─────────────────────────────────────────────────────────────────
(integer_literal) @number
(float_literal) @number
(string_literal) @string

; Backtick format strings: the text is a string, the {…} holes are code.
(format_string) @string
(format_text) @string
(format_string "`" @string)
(interpolation
  "{" @punctuation.special
  "}" @punctuation.special)

; ─── JSON Literals ────────────────────────────────────────────────────────────
(json_pair key: (string_literal) @property)
(json_pair ":" @punctuation.delimiter)

; true / false / null inside a JSON value
((json_pair value: (identifier_expr (identifier) @constant.builtin))
  (#any-of? @constant.builtin "true" "false" "null"))

((json_array (identifier_expr (identifier) @constant.builtin))
  (#any-of? @constant.builtin "true" "false" "null"))

; ─── HTML Literals ────────────────────────────────────────────────────────────
(html_tag_name) @tag
(html_attr_name) @attribute

; ─── Definitions ──────────────────────────────────────────────────────────────

; Type definition name: Bool = False + True
(type_def name: (identifier) @type)

; Named declarations: camelCase names are FFI binding aliases (functions),
; PascalCase names construct the type they are named after.
((named_def name: (identifier) @function)
  (#match? @function "^[a-z_]"))

((named_def name: (identifier) @type)
  (#match? @type "^[A-Z]"))

; Generic parameters: parallel = <T>(…) => …
(generic_param (identifier) @type.parameter)

; ─── Type Expressions ─────────────────────────────────────────────────────────

; Names inside type expressions are types (also covers dispatch-arm patterns
; and constructor inputs/returns, which are all named types).
(named_type name: (identifier) @type)

; ─── Expressions ──────────────────────────────────────────────────────────────

; camelCase method calls survive only at the FFI boundary: req.method()
(method_call method: (identifier) @function.method)

; Field access: user.Birthday, node.Rest
(field_access field: (identifier) @property)

; Constructor calls: PascalCase constructors are styled as @type because in
; Canon the constructor name IS the type name (Greeting("hi") creates a
; Greeting). camelCase calls are FFI functions.
((call name: (identifier) @function.call)
  (#match? @function.call "^[a-z_]"))

((call name: (identifier) @type)
  (#match? @type "^[A-Z]"))

; Plain identifier in expression position — PascalCase vs camelCase
((identifier_expr (identifier) @type)
  (#match? @type "^[A-Z]"))

((identifier_expr (identifier) @variable)
  (#match? @variable "^[a-z_]"))

; ─── Special Names ────────────────────────────────────────────────────────────

; Built-in types, capabilities, and well-known constructors
((identifier) @type.builtin
  (#any-of? @type.builtin
    "Bool" "Byte" "Bytes"
    "Err" "False" "Float" "Future"
    "Handle" "Html" "Int" "Json" "List"
    "Network" "Never" "None"
    "Ok" "Option" "Result" "Some"
    "Stderr" "Stdin" "Stdout" "Stream" "String"
    "True" "Unit"))
